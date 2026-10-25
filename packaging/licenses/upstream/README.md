# License texts copied from other repositories

Every library should bring its license files in the package that cargo downloads, and `generate.py` reads them from
there. A few libraries do not. For those, the text of the license is kept in this folder exactly as the authors
published it: not edited, not reformatted. `../extra.toml` says which library uses which file; this is where each
file comes from.

| Folder | Files | Copied from | Used for |
| --- | --- | --- | --- |
| `egui/` | `LICENSE-MIT`, `LICENSE-APACHE` | [emilk/egui, tag 0.37.0](https://github.com/emilk/egui/tree/0.37.0) | `ecolor`, `eframe`, `egui`, `egui-winit`, `egui_glow`, `emath`, `epaint`, `epaint_default_fonts` |
| `accesskit/` | `LICENSE-MIT`, `LICENSE-APACHE` | [AccessKit/accesskit, commit ce8164b](https://github.com/AccessKit/accesskit/tree/ce8164ba92995cfa86005b6259115e08c8244253) | `accesskit` |
| `profiling/` | `LICENSE-MIT`, `LICENSE-APACHE` | [aclysma/profiling, commit 8271551](https://github.com/aclysma/profiling/tree/8271551172eb6fa4cba47369aedd93790c623df9) | `profiling` |
| `gl-rs/` | `LICENSE` | [brendanzab/gl-rs, commit ea503e8](https://github.com/brendanzab/gl-rs/tree/ea503e8d5fb6d73c6030e6191ce738cd3bf3433e) (the same text at [commit f150967](https://github.com/brendanzab/gl-rs/tree/f150967b1c44ae888e6676f93f639ebc82771bdc)) | `gl_generator`, `khronos_api` |
| `objc2-old/` | `LICENSE.txt` | [madsmtm/objc2, commit 4fc083f](https://github.com/madsmtm/objc2/tree/4fc083f1c6d6784577e38b0ee8dbd344481e2fd2) | `block2` 0.5, `objc-sys`, `objc2` 0.5, `objc2-app-kit` 0.2, `objc2-foundation` 0.2 |
| `objc2/` | `LICENSE-MIT.txt`, `LICENSE.md` | [madsmtm/objc2, branch main](https://github.com/madsmtm/objc2/tree/main) | `block2` 0.6, `dispatch2`, `objc2` 0.6, `objc2-app-kit` 0.3, `objc2-core-foundation`, `objc2-encode`, `objc2-foundation` 0.3 |

The newer `objc2` crates were published when the repository had only a `LICENSE.md` that points to the MIT text
somewhere else; the repository has the full text now, so both files are kept.

`dispatch` is the one library with no license text anywhere (neither in its package nor in its repository). Its
`Cargo.toml` says MIT and names its author, so `THIRD-PARTY-LICENSES.md` gives the standard MIT text with that author,
and says so.
