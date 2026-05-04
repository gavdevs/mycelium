//! Verifies that the notify items used by the watcher resolve under the
//! pinned 8.2 release. notify 8.x has shuffled exports a couple of times;
//! if this test fails, src/watcher.rs needs fixups.

use notify::{recommended_watcher, EventKind, RecursiveMode};

#[test]
fn imports_compile() {
    let _ = (
        RecursiveMode::Recursive,
        EventKind::Modify(notify::event::ModifyKind::Any),
    );
    fn _coerce<F: Fn(notify::Result<notify::Event>) + Send + 'static>(_f: F) {}
    _coerce(|_| {});
    let _ = recommended_watcher::<fn(notify::Result<notify::Event>)>;
}
