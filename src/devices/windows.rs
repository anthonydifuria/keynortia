//! Keyboards on Windows.
//!
//! * The list of keyboards comes from Raw Input (`GetRawInputDeviceList`), the same API that
//!   gives their keys: `WM_INPUT` arrives even while Keynortia is in the background, and says which
//!   keyboard each key came from.
//! * An *exclusive* keyboard must not also type in the other programs. Windows has no way to take
//!   one keyboard away from the system, but a low-level keyboard hook can swallow keys, without
//!   knowing which keyboard they came from. So the hook asks Raw Input: it waits a few milliseconds
//!   for the report of the same key (see `correlate`) and swallows the key if that keyboard is an
//!   exclusive one. If Raw Input does not answer in time the key goes through: typing is never lost.
//!
//! The hook only exists while at least one keyboard is exclusive. Windows does not let a program
//! see or swallow the keys typed into an application that runs as administrator.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::mem::{size_of, size_of_val, zeroed};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant};

use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDP_BUTTON_CAPS, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_GetProductString, HidP_GetButtonCaps,
    HidP_GetCaps, HidP_Input, PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, HRAWINPUT, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RID_DEVICE_INFO, RID_DEVICE_INFO_KEYBOARD,
    RID_INPUT, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDI_DEVICEINFO, RIDI_DEVICENAME,
    RIDI_PREPARSEDDATA, RIM_TYPEKEYBOARD, RegisterRawInputDevices,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GIDC_REMOVAL, GetMessageW,
    HC_ACTION, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RI_KEY_BREAK, RI_KEY_E0, RI_KEY_E1,
    RegisterClassW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WM_APP, WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_USER, WNDCLASSW,
};

use super::correlate::{Correlator, KeyId};
use super::winpath;
use super::{
    DeviceId, KeyEvent, KeyboardInfo, OpenError, Problem, UNPLUGGED, all_playable, is_playable_code,
};

// ---------------------------------------------------------------------------------------------
// The list of keyboards
// ---------------------------------------------------------------------------------------------

/// The Raw Input handles of the keyboards.
fn keyboard_handles() -> Vec<HANDLE> {
    let item = size_of::<RAWINPUTDEVICELIST>() as u32;
    // a keyboard can be plugged in between the two calls: then the second one fails, and we try again
    for _ in 0..3 {
        let mut count: u32 = 0;
        // SAFETY: with no list, the function only writes how many devices there are
        if unsafe { GetRawInputDeviceList(null_mut(), &mut count, item) } == u32::MAX || count == 0
        {
            return Vec::new();
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        // SAFETY: `list` has room for `count` devices
        let got = unsafe { GetRawInputDeviceList(list.as_mut_ptr(), &mut count, item) };
        if got == u32::MAX {
            continue;
        }
        list.truncate(got as usize);
        return list
            .into_iter()
            .filter(|d| d.dwType == RIM_TYPEKEYBOARD)
            .map(|d| d.hDevice)
            .collect();
    }
    Vec::new()
}

/// Text up to the first 0.
fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The device path, which is also how we identify the keyboard.
fn device_path(handle: HANDLE) -> Option<String> {
    let mut length: u32 = 0;
    // SAFETY: with no buffer, the function writes the length it needs (in characters)
    unsafe { GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, null_mut(), &mut length) };
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: `buffer` has room for `length` characters
    let got = unsafe {
        GetRawInputDeviceInfoW(
            handle,
            RIDI_DEVICENAME,
            buffer.as_mut_ptr().cast(),
            &mut length,
        )
    };
    (got != u32::MAX).then(|| from_wide(&buffer))
}

fn keyboard_details(handle: HANDLE) -> Option<RID_DEVICE_INFO_KEYBOARD> {
    let mut info = RID_DEVICE_INFO {
        cbSize: size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    let mut size = info.cbSize;
    // SAFETY: `info` is as big as `size` says
    let got = unsafe {
        GetRawInputDeviceInfoW(handle, RIDI_DEVICEINFO, (&raw mut info).cast(), &mut size)
    };
    if got == u32::MAX || info.dwType != RIM_TYPEKEYBOARD {
        return None;
    }
    // SAFETY: for a keyboard the system fills in the `keyboard` variant
    Some(unsafe { info.Anonymous.keyboard })
}

/// What the HID descriptor of a keyboard says about its keys.
struct Usages {
    /// Reports all the letters (a real keyboard, not a media-keys interface).
    letters: bool,
    /// Reports the numeric keypad.
    keypad: bool,
}

fn hid_usages(handle: HANDLE) -> Option<Usages> {
    let mut size: u32 = 0;
    // SAFETY: with no buffer, the function writes the size it needs (in bytes)
    unsafe { GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    // u64 elements: the parser of the descriptor wants it aligned
    let mut data = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: `data` has room for `size` bytes
    let got = unsafe {
        GetRawInputDeviceInfoW(
            handle,
            RIDI_PREPARSEDDATA,
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if got == u32::MAX {
        return None;
    }
    let preparsed = data.as_ptr() as PHIDP_PREPARSED_DATA;
    // SAFETY: HIDP_CAPS is made of numbers: all zeros is a valid value
    let mut caps: HIDP_CAPS = unsafe { zeroed() };
    // SAFETY: `preparsed` is the descriptor the system just gave us
    if unsafe { HidP_GetCaps(preparsed, &mut caps) } != HIDP_STATUS_SUCCESS {
        return None;
    }
    let mut count = caps.NumberInputButtonCaps;
    let mut buttons = vec![HIDP_BUTTON_CAPS::default(); count as usize];
    // SAFETY: `buttons` has room for `count` items
    if count > 0
        && unsafe { HidP_GetButtonCaps(HidP_Input, buttons.as_mut_ptr(), &mut count, preparsed) }
            != HIDP_STATUS_SUCCESS
    {
        return None;
    }
    buttons.truncate(count as usize);
    let mut usages = Usages {
        letters: false,
        keypad: false,
    };
    // usage page 7 is "keyboard"; usages 0x04..=0x1D are the letters, 0x59..=0x63 the keypad
    for caps in buttons.iter().filter(|b| b.UsagePage == 7) {
        // SAFETY: the variant that is read is the one `IsRange` says is filled in
        let (min, max) = unsafe {
            if caps.IsRange {
                (caps.Anonymous.Range.UsageMin, caps.Anonymous.Range.UsageMax)
            } else {
                (caps.Anonymous.NotRange.Usage, caps.Anonymous.NotRange.Usage)
            }
        };
        usages.letters |= min <= 0x04 && max >= 0x1D;
        usages.keypad |= min <= 0x59 && max >= 0x63;
    }
    Some(usages)
}

/// The name the keyboard gives itself ("USB Gaming Keyboard").
fn product_string(path: &str) -> Option<String> {
    let wide = to_wide(path);
    // SAFETY: `wide` ends with 0. Asking for no access at all is enough to ask the device its name.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut buffer = [0u16; 126];
    // SAFETY: `handle` is open and `buffer` is as big as we say
    let ok = unsafe {
        HidD_GetProductString(
            handle,
            buffer.as_mut_ptr().cast(),
            size_of_val(&buffer) as u32,
        )
    };
    // SAFETY: `handle` was opened above and is not used again
    unsafe { CloseHandle(handle) };
    if !ok {
        return None;
    }
    let name = from_wide(&buffer).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Everything known about a keyboard; `None` if it is not one (a media-keys interface, for example).
fn probe(handle: HANDLE) -> Option<KeyboardInfo> {
    let path = device_path(handle)?;
    let details = keyboard_details(handle)?;
    let (is_keyboard, has_keypad) = match hid_usages(handle) {
        Some(usages) => (usages.letters, usages.keypad),
        // no HID descriptor (the PS/2 keyboard of a laptop): go by the number of keys
        None => (
            details.dwNumberOfKeysTotal >= 60,
            details.dwNumberOfKeysTotal >= 100,
        ),
    };
    if !is_keyboard {
        return None;
    }
    let name = product_string(&path).unwrap_or_else(|| winpath::fallback_name(&path));
    let (vendor, product) = winpath::ids(&path);
    Some(KeyboardInfo {
        id: DeviceId::new(path.clone()),
        connection: winpath::connection(&path, &name),
        technical: path,
        name,
        vendor,
        product,
        // Windows does not count the keys, and does not say which ones are really there
        reported_keys: None,
        has_keypad,
        playable: all_playable(),
    })
}

/// Keeps the list of connected keyboards, updated by asking Raw Input again.
#[derive(Default)]
pub struct Scanner {
    /// Handle of the device -> what is known about it (`None` = it is not a keyboard).
    seen: HashMap<isize, Option<KeyboardInfo>>,
}

impl Scanner {
    /// Asks again which keyboards there are. Returns true if the list changed.
    pub fn refresh(&mut self) -> bool {
        let before = self.keyboards();
        let handles = keyboard_handles();
        let present: Vec<isize> = handles.iter().map(|&h| h as isize).collect();
        self.seen.retain(|handle, _| present.contains(handle));
        for handle in handles {
            self.seen
                .entry(handle as isize)
                .or_insert_with(|| probe(handle));
        }
        self.keyboards() != before
    }

    pub fn keyboards(&self) -> Vec<KeyboardInfo> {
        let mut list: Vec<KeyboardInfo> = self.seen.values().flatten().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// Windows lets every program read the keyboards: nothing to set up.
    pub fn problem(&self) -> Option<Problem> {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// The keys
// ---------------------------------------------------------------------------------------------

/// A keyboard being read.
struct Sub {
    /// Its keys must not also type in the other programs.
    exclusive: bool,
    state: Mutex<SubState>,
}

/// Called once when reading ends: `Ok` if it was stopped, `Err` if the keyboard went away.
type OnExit = Box<dyn FnOnce(Result<(), String>) + Send>;

struct SubState {
    /// Keys held down: Windows repeats a held key, and only the first press counts.
    held: HashSet<u16>,
    on_event: Box<dyn FnMut(KeyEvent) + Send>,
    on_exit: Option<OnExit>,
}

impl Sub {
    fn state(&self) -> MutexGuard<'_, SubState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn deliver(&self, code: u16, up: bool) {
        let mut state = self.state();
        let event = if up {
            if !state.held.remove(&code) {
                return;
            }
            KeyEvent::Release(code)
        } else {
            if !state.held.insert(code) {
                return;
            }
            KeyEvent::Press(code)
        };
        (state.on_event)(event);
    }

    /// Tells the owner that reading has ended (once).
    fn finish(&self, result: Result<(), String>) {
        let on_exit = self.state().on_exit.take();
        if let Some(on_exit) = on_exit {
            on_exit(result);
        }
    }
}

/// The keyboards being read, by device path.
static SUBS: LazyLock<Mutex<HashMap<String, Arc<Sub>>>> = LazyLock::new(Default::default);

fn subs() -> MutexGuard<'static, HashMap<String, Arc<Sub>>> {
    SUBS.lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    /// Path of each device handle, remembered by the Raw Input thread.
    static NAMES: RefCell<HashMap<isize, Arc<str>>> = RefCell::new(HashMap::new());
}

fn path_of(handle: HANDLE) -> Option<Arc<str>> {
    NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some(path) = names.get(&(handle as isize)) {
            return Some(Arc::clone(path));
        }
        let path: Arc<str> = device_path(handle)?.into();
        names.insert(handle as isize, Arc::clone(&path));
        Some(path)
    })
}

/// Runs `f` without ever letting a panic cross back into Windows.
fn guard(f: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(f));
}

/// A key reported by Raw Input.
fn on_key(device: HANDLE, scan: u16, flags: u16) {
    let flags = u32::from(flags);
    let key = KeyId {
        scan,
        extended: flags & (RI_KEY_E0 | RI_KEY_E1) != 0,
        up: flags & RI_KEY_BREAK != 0,
    };
    let Some(path) = path_of(device) else {
        return;
    };
    let sub = subs().get(&*path).cloned();
    // the hook, if it is running, is waiting for this
    if HOOK_STATE.load(Ordering::Acquire) == HOOK_ON {
        CORRELATOR.push(key, sub.as_ref().is_some_and(|s| s.exclusive));
    }
    // set-1 scan codes are the evdev codes for the 45 keys; the keys with a prefix are not among them
    if let Some(sub) = sub
        && !key.extended
        && is_playable_code(scan)
    {
        sub.deliver(scan, key.up);
    }
}

fn on_raw_input(handle: HRAWINPUT) {
    // 128 bytes are plenty: a keyboard report takes 40
    let mut buffer = [0u64; 16];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: `buffer` is as big as `size` says, and aligned for a RAWINPUT
    let got = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            buffer.as_mut_ptr().cast(),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if got == u32::MAX || got == 0 {
        return;
    }
    // SAFETY: the system has just written a RAWINPUT at the start of the buffer
    let raw = unsafe { &*buffer.as_ptr().cast::<RAWINPUT>() };
    if raw.header.dwType != RIM_TYPEKEYBOARD {
        return;
    }
    // SAFETY: for a keyboard report the system fills in the `keyboard` variant
    let key = unsafe { raw.data.keyboard };
    on_key(raw.header.hDevice, key.MakeCode, key.Flags);
}

/// A keyboard was unplugged.
fn on_device_removed(handle: HANDLE) {
    let Some(path) = NAMES.with(|names| names.borrow_mut().remove(&(handle as isize))) else {
        return;
    };
    let removed = subs().remove(&*path);
    if let Some(sub) = removed {
        forget(&sub);
        sub.finish(Err(UNPLUGGED.to_string()));
    }
}

unsafe extern "system" fn raw_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INPUT => guard(|| on_raw_input(lparam as HRAWINPUT)),
        WM_INPUT_DEVICE_CHANGE if wparam as u32 == GIDC_REMOVAL => {
            guard(|| on_device_removed(lparam as HANDLE));
        }
        _ => {}
    }
    // SAFETY: the same arguments the system gave us (WM_INPUT asks for this, for cleaning up)
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// A window that nobody sees, whose only job is to receive the keys.
fn create_message_window() -> Result<HWND, String> {
    let class_name = to_wide("KeynortiaKeyboardInput");
    // SAFETY: asking for the module of the program itself
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(raw_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    // SAFETY: `class` is fully filled in and `class_name` ends with 0
    if unsafe { RegisterClassW(&class) } == 0 {
        // SAFETY: no arguments
        return Err(format!(
            "could not create the input window (error {})",
            unsafe { GetLastError() }
        ));
    }
    // SAFETY: the class was just registered; HWND_MESSAGE makes a window with no screen presence
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            null(),
        )
    };
    if window.is_null() {
        // SAFETY: no arguments
        return Err(format!(
            "could not create the input window (error {})",
            unsafe { GetLastError() }
        ));
    }
    Ok(window)
}

/// The thread that receives the keys from Raw Input.
fn raw_input_thread(ready: mpsc::Sender<Result<(), String>>) {
    let window = match create_message_window() {
        Ok(window) => window,
        Err(message) => {
            let _ = ready.send(Err(message));
            return;
        }
    };
    let keyboards = RAWINPUTDEVICE {
        // generic desktop, keyboard
        usUsagePage: 0x01,
        usUsage: 0x06,
        // keys arrive even while another program is in front; and we hear about unplugging
        dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        hwndTarget: window,
    };
    // SAFETY: one valid RAWINPUTDEVICE, and its size
    if unsafe { RegisterRawInputDevices(&keyboards, 1, size_of::<RAWINPUTDEVICE>() as u32) } == 0 {
        // SAFETY: no arguments
        let error = unsafe { GetLastError() };
        let _ = ready.send(Err(format!(
            "could not ask Windows for the keyboard input (error {error})"
        )));
        return;
    }
    let _ = ready.send(Ok(()));
    let mut message = MSG::default();
    // SAFETY: the usual message loop, for the window of this thread
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        // SAFETY: `message` was just filled in by GetMessageW
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The hook that keeps exclusive keyboards from typing
// ---------------------------------------------------------------------------------------------

/// Keys reported by Raw Input, for the hook to look up.
static CORRELATOR: Correlator = Correlator::new();
/// How many of the keyboards being read are exclusive: the hook runs while there is at least one.
static EXCLUSIVE: AtomicUsize = AtomicUsize::new(0);
/// Thread that owns the hook (0 = not started).
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);

const HOOK_OFF: u32 = 0;
const HOOK_PENDING: u32 = 1;
const HOOK_ON: u32 = 2;
const HOOK_FAILED: u32 = 3;
static HOOK_STATE: AtomicU32 = AtomicU32::new(HOOK_OFF);
static HOOK_ERROR: AtomicU32 = AtomicU32::new(0);

/// Message to the hook thread: install or remove the hook, depending on how many keyboards are exclusive.
const MSG_SYNC: u32 = WM_APP + 1;

/// How long the hook waits for Raw Input to say which keyboard a key came from.
const WAIT: Duration = Duration::from_millis(20);
/// Consecutive keys nobody reported, with none ever reported, before the hook gives up.
const GIVE_UP_AFTER: usize = 8;
static MISSES: AtomicUsize = AtomicUsize::new(0);
/// Raw Input has answered the hook at least once: the two do talk.
static ANSWERED: AtomicBool = AtomicBool::new(false);
/// The hook cannot work on this system (Raw Input never answers): it lets every key through.
static GAVE_UP: AtomicBool = AtomicBool::new(false);

/// True if the key is to be swallowed.
fn swallow(key: &KBDLLHOOKSTRUCT) -> bool {
    let id = KeyId {
        scan: key.scanCode as u16,
        extended: key.flags & LLKHF_EXTENDED != 0,
        up: key.flags & LLKHF_UP != 0,
    };
    match CORRELATOR.take(id, WAIT) {
        Some(exclusive) => {
            ANSWERED.store(true, Ordering::Relaxed);
            MISSES.store(0, Ordering::Relaxed);
            exclusive
        }
        None => {
            // either a key that does not come from a keyboard Raw Input knows, or Raw Input
            // is not answering at all. In the second case, stop making every key wait.
            if !ANSWERED.load(Ordering::Relaxed)
                && MISSES.fetch_add(1, Ordering::Relaxed) + 1 >= GIVE_UP_AFTER
            {
                GAVE_UP.store(true, Ordering::Relaxed);
            }
            false
        }
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && EXCLUSIVE.load(Ordering::Relaxed) > 0
        && !GAVE_UP.load(Ordering::Relaxed)
    {
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT that lives during this call
        let key = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // keys sent by programs (not typed on a keyboard) are none of our business
        if key.flags & LLKHF_INJECTED == 0
            && matches!(catch_unwind(AssertUnwindSafe(|| swallow(key))), Ok(true))
        {
            return 1;
        }
    }
    // SAFETY: the same arguments the system gave us
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

/// The thread that owns the hook (a hook needs a thread that handles messages).
fn hook_thread(ready: mpsc::Sender<u32>) {
    let mut message = MSG::default();
    // SAFETY: peeking for a message nobody sends creates the message queue of the thread,
    // so that messages posted to it before the first GetMessage are not lost
    unsafe { PeekMessageW(&mut message, null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
    // SAFETY: no arguments
    let _ = ready.send(unsafe { GetCurrentThreadId() });

    let mut hook: HHOOK = null_mut();
    // SAFETY: the usual message loop
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        if message.message != MSG_SYNC {
            // SAFETY: `message` was just filled in by GetMessageW
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            continue;
        }
        let wanted = EXCLUSIVE.load(Ordering::SeqCst) > 0;
        if wanted && hook.is_null() {
            MISSES.store(0, Ordering::Relaxed);
            ANSWERED.store(false, Ordering::Relaxed);
            GAVE_UP.store(false, Ordering::Relaxed);
            // SAFETY: a low-level hook needs no DLL: the module of the program is enough
            hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(null()), 0)
            };
            if hook.is_null() {
                // SAFETY: no arguments
                HOOK_ERROR.store(unsafe { GetLastError() }, Ordering::SeqCst);
                HOOK_STATE.store(HOOK_FAILED, Ordering::SeqCst);
            } else {
                HOOK_STATE.store(HOOK_ON, Ordering::SeqCst);
            }
        } else if wanted {
            HOOK_STATE.store(HOOK_ON, Ordering::SeqCst);
        } else if !hook.is_null() {
            // SAFETY: `hook` was returned by SetWindowsHookExW and is not used again
            unsafe { UnhookWindowsHookEx(hook) };
            hook = null_mut();
            HOOK_STATE.store(HOOK_OFF, Ordering::SeqCst);
            CORRELATOR.clear();
        }
    }
}

fn post_sync() -> bool {
    let thread = HOOK_THREAD.load(Ordering::SeqCst);
    // SAFETY: posting a message with no data to a thread of this program
    thread != 0 && unsafe { PostThreadMessageW(thread, MSG_SYNC, 0, 0) } != 0
}

/// One more keyboard is exclusive: makes sure the hook is installed.
fn acquire_exclusive() -> Result<(), String> {
    EXCLUSIVE.fetch_add(1, Ordering::SeqCst);
    if HOOK_STATE.load(Ordering::SeqCst) != HOOK_ON {
        HOOK_STATE.store(HOOK_PENDING, Ordering::SeqCst);
    }
    let result = if post_sync() {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match HOOK_STATE.load(Ordering::SeqCst) {
                HOOK_ON => break Ok(()),
                HOOK_FAILED => {
                    break Err(format!(
                        "could not install the keyboard hook (Windows error {})",
                        HOOK_ERROR.load(Ordering::SeqCst)
                    ));
                }
                _ if Instant::now() > deadline => {
                    break Err("the keyboard hook did not start".to_string());
                }
                _ => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    } else {
        Err("the keyboard hook thread is not running".to_string())
    };
    if result.is_err() {
        release_exclusive();
    }
    result
}

/// One keyboard is no longer exclusive: removes the hook if it was the last.
fn release_exclusive() {
    EXCLUSIVE.fetch_sub(1, Ordering::SeqCst);
    post_sync();
}

/// A keyboard stops being read.
fn forget(sub: &Sub) {
    if sub.exclusive {
        release_exclusive();
    }
}

// ---------------------------------------------------------------------------------------------
// Starting and stopping
// ---------------------------------------------------------------------------------------------

/// The two threads are started the first time a keyboard is read.
fn ensure_started() -> Result<(), OpenError> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            let (raw_tx, raw_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("keynortia-rawinput".into())
                .spawn(move || raw_input_thread(raw_tx))
                .map_err(|e| e.to_string())?;
            raw_rx
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| "the keyboard input thread did not start".to_string())??;

            let (hook_tx, hook_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("keynortia-hook".into())
                .spawn(move || hook_thread(hook_tx))
                .map_err(|e| e.to_string())?;
            let thread = hook_rx
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| "the keyboard hook thread did not start".to_string())?;
            HOOK_THREAD.store(thread, Ordering::SeqCst);
            Ok(())
        })
        .clone()
        .map_err(OpenError::other)
}

/// A keyboard being read; stop reading it by dropping this (or with `stop`).
pub struct Reader {
    id: String,
    exclusive: bool,
    /// Why the keyboard is not exclusive although it was asked to be.
    warning: Option<String>,
}

impl Reader {
    /// Stops reading the keyboard.
    pub fn stop(self) {
        drop(self);
    }

    /// Something that does not work as asked, to show next to "listening".
    pub fn warning(&self) -> Option<String> {
        if let Some(warning) = &self.warning {
            return Some(warning.clone());
        }
        (self.exclusive && GAVE_UP.load(Ordering::Relaxed)).then(|| {
            "exclusive mode does not work on this system: this keyboard also types".to_string()
        })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let removed = subs().remove(&self.id);
        if let Some(sub) = removed {
            forget(&sub);
            sub.finish(Ok(()));
        }
    }
}

/// Starts reading keyboard `info`.
///
/// With `grab` the keyboard becomes exclusive: its keys no longer reach any other program. If
/// that cannot be done the keyboard is read anyway, and `Reader::warning` says so.
pub fn spawn_reader(
    info: &KeyboardInfo,
    grab: bool,
    on_event: impl FnMut(KeyEvent) + Send + 'static,
    on_exit: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<Reader, OpenError> {
    ensure_started()?;
    let id = info.id.as_str().to_string();
    let mut warning = None;
    let mut exclusive = false;
    if grab {
        match acquire_exclusive() {
            Ok(()) => exclusive = true,
            Err(message) => warning = Some(format!("{message}: this keyboard also types")),
        }
    }
    let sub = Arc::new(Sub {
        exclusive,
        state: Mutex::new(SubState {
            held: HashSet::new(),
            on_event: Box::new(on_event),
            on_exit: Some(Box::new(on_exit)),
        }),
    });
    let replaced = subs().insert(id.clone(), sub);
    if let Some(old) = replaced {
        forget(&old);
    }
    Ok(Reader {
        id,
        exclusive,
        warning,
    })
}
