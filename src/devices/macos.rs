//! Keyboards on macOS, through IOKit HID.
//!
//! * The list of keyboards comes from an `IOHIDManager` that lives in a thread of its own. The
//!   manager is only *scheduled*, never opened: opening it would open (and read) every keyboard.
//! * To read a keyboard, a fresh `IOHIDDevice` is made from its entry in the IORegistry and opened.
//!   Its values then arrive in a thread of their own, one per key (1 = pressed, 0 = released).
//!   With *exclusive* the device is *seized*: macOS stops handing its keys to the other programs.
//!   macOS may refuse (the keyboard of a laptop, say): then the keyboard is read anyway and
//!   `Reader::warning` says that it also types.
//! * Reading keys needs the "Input Monitoring" permission (System Settings, Privacy & Security).
//!   Without it opening a keyboard fails, and the error says what to do.
//!
//! Only a Mac can try this out. The logic that does not touch the system is in `hid_usage`, which
//! is tested everywhere.

use std::collections::{BTreeSet, HashMap};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{CFRunLoop, CFRunLoopRunResult, kCFRunLoopDefaultMode};
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFRelease, kCFAllocatorDefault};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::set::{CFSetGetCount, CFSetGetValues};

use super::hid_usage::{self, KeyTracker};
use super::{
    DeviceId, KeyEvent, KeyboardInfo, OpenError, OpenErrorKind, Problem, UNPLUGGED, all_playable,
};

// ---------------------------------------------------------------------------------------------
// IOKit
// ---------------------------------------------------------------------------------------------

/// The few functions of IOKit that are needed (`IOKit/hid/*.h`), declared by hand.
#[allow(non_camel_case_types)]
mod iokit {
    use std::ffi::c_void;

    use core_foundation_sys::array::CFArrayRef;
    use core_foundation_sys::base::{Boolean, CFAllocatorRef, CFIndex, CFTypeRef};
    use core_foundation_sys::dictionary::{CFDictionaryRef, CFMutableDictionaryRef};
    use core_foundation_sys::runloop::CFRunLoopRef;
    use core_foundation_sys::set::CFSetRef;
    use core_foundation_sys::string::CFStringRef;

    pub type IOReturn = i32;
    pub type IOOptionBits = u32;
    /// A Mach port.
    pub type io_service_t = u32;

    // all of these are pointers to opaque objects; CoreFoundation objects, so `CFRelease` works on them
    pub type IOHIDManagerRef = CFTypeRef;
    pub type IOHIDDeviceRef = CFTypeRef;
    pub type IOHIDElementRef = CFTypeRef;
    pub type IOHIDValueRef = CFTypeRef;

    pub type IOHIDValueCallback = extern "C" fn(
        context: *mut c_void,
        result: IOReturn,
        sender: *mut c_void,
        value: IOHIDValueRef,
    );
    pub type IOHIDCallback =
        extern "C" fn(context: *mut c_void, result: IOReturn, sender: *mut c_void);

    /// `kIOHIDOptionsTypeSeizeDevice`: open the device for us alone.
    pub const SEIZE_DEVICE: IOOptionBits = 1;

    pub const SUCCESS: IOReturn = 0;
    // `iokit_common_err(n)` is 0xe0000000 | n
    pub const NOT_PRIVILEGED: IOReturn = 0xe000_02c1_u32 as i32;
    pub const EXCLUSIVE_ACCESS: IOReturn = 0xe000_02c5_u32 as i32;
    pub const NOT_PERMITTED: IOReturn = 0xe000_02e2_u32 as i32;

    /// `kIOHIDRequestTypeListenEvent`: the Input Monitoring permission.
    pub const REQUEST_LISTEN_EVENT: u32 = 1;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        pub fn IOHIDManagerCreate(
            allocator: CFAllocatorRef,
            options: IOOptionBits,
        ) -> IOHIDManagerRef;
        pub fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
        pub fn IOHIDManagerCopyDevices(manager: IOHIDManagerRef) -> CFSetRef;
        pub fn IOHIDManagerScheduleWithRunLoop(
            manager: IOHIDManagerRef,
            run_loop: CFRunLoopRef,
            mode: CFStringRef,
        );

        pub fn IOHIDDeviceCreate(
            allocator: CFAllocatorRef,
            service: io_service_t,
        ) -> IOHIDDeviceRef;
        pub fn IOHIDDeviceGetService(device: IOHIDDeviceRef) -> io_service_t;
        pub fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
        pub fn IOHIDDeviceClose(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
        pub fn IOHIDDeviceConformsTo(
            device: IOHIDDeviceRef,
            usage_page: u32,
            usage: u32,
        ) -> Boolean;
        pub fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
        pub fn IOHIDDeviceCopyMatchingElements(
            device: IOHIDDeviceRef,
            matching: CFDictionaryRef,
            options: IOOptionBits,
        ) -> CFArrayRef;
        pub fn IOHIDDeviceRegisterInputValueCallback(
            device: IOHIDDeviceRef,
            callback: Option<IOHIDValueCallback>,
            context: *mut c_void,
        );
        pub fn IOHIDDeviceRegisterRemovalCallback(
            device: IOHIDDeviceRef,
            callback: Option<IOHIDCallback>,
            context: *mut c_void,
        );
        pub fn IOHIDDeviceScheduleWithRunLoop(
            device: IOHIDDeviceRef,
            run_loop: CFRunLoopRef,
            mode: CFStringRef,
        );
        pub fn IOHIDDeviceUnscheduleFromRunLoop(
            device: IOHIDDeviceRef,
            run_loop: CFRunLoopRef,
            mode: CFStringRef,
        );

        pub fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
        pub fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> CFIndex;
        pub fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
        pub fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;

        /// Asks the user for the permission, and waits for the answer.
        pub fn IOHIDRequestAccess(request_type: u32) -> Boolean;

        pub fn IORegistryEntryGetRegistryEntryID(entry: io_service_t, id: *mut u64) -> IOReturn;
        pub fn IORegistryEntryIDMatching(id: u64) -> CFMutableDictionaryRef;
        pub fn IOServiceGetMatchingService(
            main_port: u32,
            matching: CFDictionaryRef,
        ) -> io_service_t;
        pub fn IOObjectRelease(object: io_service_t) -> IOReturn;
    }
}

use iokit::{
    EXCLUSIVE_ACCESS, IOHIDDeviceClose, IOHIDDeviceConformsTo, IOHIDDeviceCopyMatchingElements,
    IOHIDDeviceCreate, IOHIDDeviceGetProperty, IOHIDDeviceGetService, IOHIDDeviceOpen,
    IOHIDDeviceRef, IOHIDDeviceRegisterInputValueCallback, IOHIDDeviceRegisterRemovalCallback,
    IOHIDDeviceScheduleWithRunLoop, IOHIDDeviceUnscheduleFromRunLoop, IOHIDElementGetUsage,
    IOHIDElementGetUsagePage, IOHIDManagerCopyDevices, IOHIDManagerCreate, IOHIDManagerRef,
    IOHIDManagerScheduleWithRunLoop, IOHIDManagerSetDeviceMatching, IOHIDRequestAccess,
    IOHIDValueGetElement, IOHIDValueGetIntegerValue, IOHIDValueRef, IOObjectRelease, IOOptionBits,
    IORegistryEntryGetRegistryEntryID, IORegistryEntryIDMatching, IOReturn,
    IOServiceGetMatchingService, NOT_PERMITTED, NOT_PRIVILEGED, REQUEST_LISTEN_EVENT, SEIZE_DEVICE,
    SUCCESS,
};

// ---------------------------------------------------------------------------------------------
// The list of keyboards
// ---------------------------------------------------------------------------------------------

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the scanning thread has found so far.
#[derive(Default)]
struct Scan {
    /// The first pass is over.
    ready: bool,
    /// Goes up each time the list changes.
    generation: u64,
    keyboards: Vec<KeyboardInfo>,
}

#[derive(Default)]
struct ScanShared {
    scan: Mutex<Scan>,
    ready: Condvar,
}

impl ScanShared {
    fn publish(&self, keyboards: Vec<KeyboardInfo>) {
        let mut scan = lock(&self.scan);
        if !scan.ready || scan.keyboards != keyboards {
            scan.keyboards = keyboards;
            scan.generation += 1;
        }
        scan.ready = true;
        drop(scan);
        self.ready.notify_all();
    }
}

static SCAN: OnceLock<Arc<ScanShared>> = OnceLock::new();

/// The scanning thread starts the first time the keyboards are asked for.
fn scan_shared() -> &'static Arc<ScanShared> {
    SCAN.get_or_init(|| {
        let shared = Arc::new(ScanShared::default());
        let worker = Arc::clone(&shared);
        let started = std::thread::Builder::new()
            .name("keynortia-keyboards".into())
            .spawn(move || scan_thread(&worker));
        if started.is_err() {
            // no keyboards will ever show up, but nobody has to wait for them
            shared.publish(Vec::new());
        }
        shared
    })
}

/// Keeps the list of connected keyboards, which a thread of its own asks IOKit for.
#[derive(Default)]
pub struct Scanner {
    /// The generation of the list we hold.
    seen: u64,
    list: Vec<KeyboardInfo>,
}

impl Scanner {
    /// Takes the latest list from the scanning thread. Returns true if it changed.
    pub fn refresh(&mut self) -> bool {
        let shared = scan_shared();
        let mut scan = lock(&shared.scan);
        if !scan.ready {
            // the very first time: give the thread a moment to look around
            scan = shared
                .ready
                .wait_timeout_while(scan, Duration::from_secs(2), |scan| !scan.ready)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if scan.generation == self.seen {
            return false;
        }
        self.seen = scan.generation;
        self.list = scan.keyboards.clone();
        true
    }

    pub fn keyboards(&self) -> Vec<KeyboardInfo> {
        self.list.clone()
    }

    /// Listing the keyboards needs no permission, so there is nothing to explain here: the
    /// permission is asked for, and explained, when a keyboard is opened.
    pub fn problem(&self) -> Option<Problem> {
        None
    }
}

/// How often the scanning thread looks at the list of devices again.
const SCAN_EVERY: Duration = Duration::from_millis(500);

fn scan_thread(shared: &ScanShared) {
    // SAFETY: plain IOKit calls; the manager is made here and used only in this thread
    let manager = unsafe { IOHIDManagerCreate(kCFAllocatorDefault, 0) };
    if manager.is_null() {
        shared.publish(Vec::new());
        return;
    }
    let run_loop = CFRunLoop::get_current();
    let matching = keyboard_matching();
    // SAFETY: `manager` is alive, and so are the dictionary and the run loop for these calls
    unsafe {
        IOHIDManagerSetDeviceMatching(manager, matching.as_concrete_TypeRef());
        IOHIDManagerScheduleWithRunLoop(
            manager,
            run_loop.as_concrete_TypeRef(),
            kCFRunLoopDefaultMode,
        );
    }

    // IORegistry entry -> what is known about it (`None` = it is not a keyboard)
    let mut known: HashMap<u64, Option<KeyboardInfo>> = HashMap::new();
    let mut wait = Duration::from_millis(100);
    // Every Mac has a keyboard: if there is none at first, the manager is probably still looking
    // (the first list is held back for up to a second)
    let mut looks_left = 10;
    loop {
        // The run loop is where the manager learns that keyboards come and go. A first, short
        // wait lets it see the keyboards that are already there.
        // SAFETY: a constant string of CoreFoundation
        let mode = unsafe { kCFRunLoopDefaultMode };
        if matches!(
            CFRunLoop::run_in_mode(mode, wait, false),
            CFRunLoopRunResult::Finished
        ) {
            // nothing to wait for (it should not happen): do not spin
            std::thread::sleep(wait);
        }

        let devices = copy_devices(manager);
        let mut present = Vec::with_capacity(devices.len());
        for device in &devices {
            let device = device.as_CFTypeRef();
            if let Some(entry) = entry_id(device) {
                present.push(entry);
                known
                    .entry(entry)
                    .or_insert_with(|| describe(device, entry));
            }
        }
        known.retain(|entry, _| present.contains(entry));

        let mut keyboards: Vec<KeyboardInfo> = known.values().flatten().cloned().collect();
        keyboards.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        if keyboards.is_empty() && looks_left > 0 {
            looks_left -= 1;
            continue;
        }
        looks_left = 0;
        wait = SCAN_EVERY;
        shared.publish(keyboards);
    }
}

/// "Generic desktop page, keyboard usage": what the manager is asked to find.
fn keyboard_matching() -> CFDictionary<CFType, CFType> {
    CFDictionary::from_CFType_pairs(&[
        (
            CFString::new("DeviceUsagePage").as_CFType(),
            CFNumber::from(1i32).as_CFType(),
        ),
        (
            CFString::new("DeviceUsage").as_CFType(),
            CFNumber::from(6i32).as_CFType(),
        ),
    ])
}

/// The devices the manager knows about (each one kept alive by the returned value).
fn copy_devices(manager: IOHIDManagerRef) -> Vec<CFType> {
    // SAFETY: `manager` is alive. The function follows the "copy" rule: we own the set.
    let set = unsafe { IOHIDManagerCopyDevices(manager) };
    if set.is_null() {
        return Vec::new();
    }
    // SAFETY: `set` is a CFSet; `values` has room for as many pointers as the set holds
    let devices = unsafe {
        let count = CFSetGetCount(set).max(0) as usize;
        let mut values: Vec<*const c_void> = vec![null(); count];
        CFSetGetValues(set, values.as_mut_ptr());
        values
            .into_iter()
            .filter(|value| !value.is_null())
            // the set owns its members: take a reference of our own to each one
            .map(|value| CFType::wrap_under_get_rule(value))
            .collect()
    };
    // SAFETY: we own `set` and do not use it again
    unsafe { CFRelease(set.cast()) };
    devices
}

/// The number of the device's entry in the IORegistry, which stays the same while it is plugged in.
fn entry_id(device: IOHIDDeviceRef) -> Option<u64> {
    // SAFETY: `device` is alive
    let service = unsafe { IOHIDDeviceGetService(device) };
    if service == 0 {
        return None;
    }
    let mut id = 0u64;
    // SAFETY: `id` is a valid place for the answer; the service is not ours to release
    (unsafe { IORegistryEntryGetRegistryEntryID(service, &mut id) } == SUCCESS).then_some(id)
}

fn property(device: IOHIDDeviceRef, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    // SAFETY: `device` is alive and `key` is a CFString
    let value = unsafe { IOHIDDeviceGetProperty(device, key.as_concrete_TypeRef()) };
    // SAFETY: "get" rule: the property belongs to the device, so we take a reference of our own
    (!value.is_null()).then(|| unsafe { CFType::wrap_under_get_rule(value) })
}

fn string_property(device: IOHIDDeviceRef, key: &str) -> Option<String> {
    let value = property(device, key)?;
    value.downcast::<CFString>().map(|s| s.to_string())
}

fn number_property(device: IOHIDDeviceRef, key: &str) -> Option<i64> {
    let value = property(device, key)?;
    value.downcast::<CFNumber>()?.to_i64()
}

fn bool_property(device: IOHIDDeviceRef, key: &str) -> Option<bool> {
    let value = property(device, key)?;
    value.downcast::<CFBoolean>().map(bool::from)
}

/// The usages of the keyboard page that the device can report. IOKit makes one element for each
/// usage (even for the keys that a report lists in an array), so these are the keys it has.
/// `None` if the system does not say.
fn key_usages(device: IOHIDDeviceRef) -> Option<BTreeSet<u32>> {
    // SAFETY: `device` is alive; no matching dictionary = all the elements
    let array = unsafe { IOHIDDeviceCopyMatchingElements(device, null(), 0) };
    if array.is_null() {
        return None;
    }
    // SAFETY: the function follows the "copy" rule: we own the array
    let array: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(array) };
    let mut usages = BTreeSet::new();
    for element in array.iter() {
        let element = element.as_CFTypeRef();
        // SAFETY: the elements live as long as the array
        let (page, usage) = unsafe {
            (
                IOHIDElementGetUsagePage(element),
                IOHIDElementGetUsage(element),
            )
        };
        if page == hid_usage::KEYBOARD_PAGE {
            usages.insert(usage);
        }
    }
    Some(usages)
}

/// Everything known about a device; `None` if it is not a keyboard (a media-keys interface, say).
fn describe(device: IOHIDDeviceRef, entry: u64) -> Option<KeyboardInfo> {
    // SAFETY: `device` is alive
    if unsafe { IOHIDDeviceConformsTo(device, 0x01, 0x06) } == 0 {
        return None;
    }
    let (playable, has_keypad) = match key_usages(device) {
        Some(usages) if !usages.is_empty() => {
            if !hid_usage::has_letters(&usages) {
                return None;
            }
            (
                hid_usage::playable_codes(&usages),
                hid_usage::has_keypad(&usages),
            )
        }
        // the system does not say: take it for a keyboard that has all the keys
        _ => (all_playable(), false),
    };

    let name = string_property(device, "Product")
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Keyboard".to_string());
    let transport = string_property(device, "Transport").unwrap_or_default();
    let built_in = bool_property(device, "Built-In").unwrap_or(false);
    let number = |key| {
        number_property(device, key)
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(0)
    };

    let mut technical = format!("IOKit entry {entry:#x}");
    if !transport.is_empty() {
        technical.push_str(&format!(" · {transport}"));
    }
    if let Some(location) = number_property(device, "LocationID") {
        technical.push_str(&format!(" · location {location:#x}"));
    }
    Some(KeyboardInfo {
        id: DeviceId::new(hid_usage::id_of_entry(entry)),
        connection: hid_usage::connection(&transport, built_in, &name),
        technical,
        vendor: number("VendorID"),
        product: number("ProductID"),
        name,
        // macOS does not count the keys
        reported_keys: None,
        has_keypad,
        playable,
    })
}

// ---------------------------------------------------------------------------------------------
// The keys
// ---------------------------------------------------------------------------------------------

type OnEvent = Box<dyn FnMut(KeyEvent) + Send>;
type OnExit = Box<dyn FnOnce(Result<(), String>) + Send>;
/// How the reading thread says whether the keyboard could be opened (and, if so, the warning).
type Ready = mpsc::SyncSender<Result<Option<String>, OpenError>>;

/// What the callbacks of IOKit work with.
struct Callbacks {
    on_event: OnEvent,
    keys: KeyTracker,
    unplugged: Arc<AtomicBool>,
}

/// A value arrived: a key went down or up.
extern "C" fn on_value(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    value: IOHIDValueRef,
) {
    // a panic must not unwind into IOKit
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `context` is the `Callbacks` that `read_keyboard` registered with this callback:
        // it lives until the callback is removed, and both run in the same thread
        let callbacks = unsafe { &mut *context.cast::<Callbacks>() };
        // SAFETY: `value` is alive during the callback
        let element = unsafe { IOHIDValueGetElement(value) };
        if element.is_null() {
            return;
        }
        // SAFETY: as above, `element` belongs to `value`
        let (page, usage, down) = unsafe {
            (
                IOHIDElementGetUsagePage(element),
                IOHIDElementGetUsage(element),
                IOHIDValueGetIntegerValue(value) != 0,
            )
        };
        if let Some(event) = callbacks.keys.update(page, usage, down) {
            (callbacks.on_event)(event);
        }
    }));
}

/// The keyboard was unplugged.
extern "C" fn on_removal(context: *mut c_void, _result: IOReturn, _sender: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as in `on_value`
        let callbacks = unsafe { &*context.cast::<Callbacks>() };
        callbacks.unplugged.store(true, Ordering::SeqCst);
        CFRunLoop::get_current().stop();
    }));
}

/// The keyboard as `open` leaves it.
struct Opened {
    device: CFType,
    /// The options it was opened with: closing wants the same ones.
    options: IOOptionBits,
    /// Why the keyboard is not exclusive although it was asked to be.
    warning: Option<String>,
}

/// A device of our own for the keyboard with that IORegistry entry.
fn create_device(entry: u64) -> Option<CFType> {
    // SAFETY: plain IOKit calls. `IOServiceGetMatchingService` consumes the matching dictionary;
    // the service is released once the device has been made from it.
    unsafe {
        let matching = IORegistryEntryIDMatching(entry);
        if matching.is_null() {
            return None;
        }
        // 0 = the default port
        let service = IOServiceGetMatchingService(0, matching as CFDictionaryRef);
        if service == 0 {
            return None;
        }
        let device = IOHIDDeviceCreate(kCFAllocatorDefault, service);
        IOObjectRelease(service);
        (!device.is_null()).then(|| CFType::wrap_under_create_rule(device))
    }
}

/// Asks macOS for the Input Monitoring permission (once). The call waits for the answer to the
/// dialog, so it goes to a thread of its own.
fn ask_for_permission() {
    static ASKED: AtomicBool = AtomicBool::new(false);
    if ASKED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("keynortia-permission".into())
        .spawn(|| {
            // SAFETY: a plain call, with nothing to point to
            unsafe { IOHIDRequestAccess(REQUEST_LISTEN_EVENT) };
        });
}

/// The words for an error on opening a keyboard.
fn open_error(code: IOReturn) -> OpenError {
    match code {
        NOT_PERMITTED | NOT_PRIVILEGED => {
            ask_for_permission();
            OpenError::other(
                "macOS has not allowed Keynortia to read the keyboards. Open System Settings > \
                 Privacy & Security > Input Monitoring and switch Keynortia on (if you started it \
                 from a terminal, switch that terminal on), then quit and reopen it.",
            )
        }
        EXCLUSIVE_ACCESS => OpenError {
            kind: OpenErrorKind::Busy,
            message:
                "already taken by another program (a key remapper such as Karabiner-Elements, \
                      say): free this keyboard there"
                    .to_string(),
        },
        other => OpenError::other(format!(
            "macOS could not open the keyboard (error {other:#x})"
        )),
    }
}

/// Opens the keyboard; with `grab`, exclusively if macOS lets us.
fn open(entry: u64, grab: bool) -> Result<Opened, OpenError> {
    let device =
        create_device(entry).ok_or_else(|| OpenError::other("the keyboard is no longer there"))?;
    let raw = device.as_CFTypeRef();
    let mut warning = None;
    if grab {
        // SAFETY: `raw` is alive: `device` owns it
        if unsafe { IOHIDDeviceOpen(raw, SEIZE_DEVICE) } == SUCCESS {
            return Ok(Opened {
                device,
                options: SEIZE_DEVICE,
                warning,
            });
        }
        warning =
            Some("macOS did not allow exclusive mode for this keyboard: it also types".to_string());
    }
    // SAFETY: as above
    let code = unsafe { IOHIDDeviceOpen(raw, 0) };
    if code != SUCCESS {
        return Err(open_error(code));
    }
    Ok(Opened {
        device,
        options: 0,
        warning,
    })
}

/// Reads the keyboard until `stop` is set or the keyboard goes away. Runs in a thread of its own.
fn read_keyboard(
    entry: u64,
    grab: bool,
    on_event: OnEvent,
    on_exit: OnExit,
    stop: &AtomicBool,
    ready: &Ready,
) {
    let opened = match open(entry, grab) {
        Ok(opened) => opened,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let device = opened.device.as_CFTypeRef();
    let unplugged = Arc::new(AtomicBool::new(false));
    let callbacks = Box::into_raw(Box::new(Callbacks {
        on_event,
        keys: KeyTracker::default(),
        unplugged: Arc::clone(&unplugged),
    }));
    let run_loop = CFRunLoop::get_current();
    // SAFETY: a constant string of CoreFoundation
    let mode = unsafe { kCFRunLoopDefaultMode };
    // SAFETY: `device` stays alive (`opened` owns it) until the callbacks are removed below, and
    // `callbacks` is freed only after that
    unsafe {
        IOHIDDeviceRegisterInputValueCallback(device, Some(on_value), callbacks.cast());
        IOHIDDeviceRegisterRemovalCallback(device, Some(on_removal), callbacks.cast());
        IOHIDDeviceScheduleWithRunLoop(device, run_loop.as_concrete_TypeRef(), mode);
    }
    let _ = ready.send(Ok(opened.warning.clone()));

    // Waiting in slices of 0.1 s, so that `stop` is noticed. The keys are handled as they arrive.
    let mut nothing_to_wait_for = 0;
    while !stop.load(Ordering::Relaxed) && !unplugged.load(Ordering::SeqCst) {
        match CFRunLoop::run_in_mode(mode, Duration::from_millis(100), false) {
            // IOKit takes the keyboard out of the run loop when it goes away
            CFRunLoopRunResult::Finished => {
                nothing_to_wait_for += 1;
                if nothing_to_wait_for >= 10 {
                    unplugged.store(true, Ordering::SeqCst);
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            _ => nothing_to_wait_for = 0,
        }
    }

    // SAFETY: undoes what was done above, in the opposite order. After the callbacks are removed
    // nothing can reach `callbacks` any more, so it can be freed.
    unsafe {
        IOHIDDeviceUnscheduleFromRunLoop(device, run_loop.as_concrete_TypeRef(), mode);
        IOHIDDeviceRegisterInputValueCallback(device, None, null_mut());
        IOHIDDeviceRegisterRemovalCallback(device, None, null_mut());
        IOHIDDeviceClose(device, opened.options);
        drop(Box::from_raw(callbacks));
    }
    drop(opened);
    on_exit(if unplugged.load(Ordering::SeqCst) {
        Err(UNPLUGGED.to_string())
    } else {
        Ok(())
    });
}

/// A keyboard being read.
pub struct Reader {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Why the keyboard is not exclusive although it was asked to be.
    warning: Option<String>,
}

impl Reader {
    /// Stops reading and waits for the thread to end (at most ~0.1 s).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    /// Something that does not work as asked, to show next to "listening".
    pub fn warning(&self) -> Option<String> {
        self.warning.clone()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Starts reading keyboard `info`. Errors on opening come back at once; later ones (the keyboard
/// being unplugged) are passed to `on_exit` when the thread ends.
///
/// With `grab` the keyboard becomes exclusive: its keys no longer reach any other program. If
/// macOS does not allow that the keyboard is read anyway, and `Reader::warning` says so.
pub fn spawn_reader(
    info: &KeyboardInfo,
    grab: bool,
    on_event: impl FnMut(KeyEvent) + Send + 'static,
    on_exit: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<Reader, OpenError> {
    let entry = hid_usage::entry_of_id(info.id.as_str())
        .ok_or_else(|| OpenError::other("unknown keyboard"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let handle = std::thread::Builder::new()
        .name("keynortia-keyboard".into())
        .spawn(move || {
            read_keyboard(
                entry,
                grab,
                Box::new(on_event),
                Box::new(on_exit),
                &stop_flag,
                &ready_tx,
            );
        })
        .map_err(|e| OpenError::other(format!("could not start reading the keyboard: {e}")))?;
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(warning)) => Ok(Reader {
            stop,
            handle: Some(handle),
            warning,
        }),
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            // it should not take so long: let the thread end by itself, whenever it does
            stop.store(true, Ordering::Relaxed);
            Err(OpenError::other("the keyboard did not answer"))
        }
    }
}
