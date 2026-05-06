use surch_store::lock::{LockError, MemoryLockFactory};

#[test]
fn memory_lock_factory_reacquires_after_guard_drop() {
    let factory = MemoryLockFactory::new();

    let guard = factory.obtain_lock("write.lock").expect("acquire lock");

    drop(guard);

    factory
        .obtain_lock("write.lock")
        .expect("reacquire released lock");
}

#[test]
fn memory_lock_factory_rejects_second_acquire_while_guard_lives() {
    let factory = MemoryLockFactory::new();
    let _guard = factory.obtain_lock("write.lock").expect("acquire lock");

    assert!(matches!(
        factory.obtain_lock("write.lock"),
        Err(LockError::AlreadyLocked { name }) if name == "write.lock"
    ));
}

#[test]
fn memory_lock_factory_tracks_lock_names_independently() {
    let factory = MemoryLockFactory::new();
    let _write_guard = factory
        .obtain_lock("write.lock")
        .expect("acquire write lock");

    factory
        .obtain_lock("merge.lock")
        .expect("acquire independent merge lock");
}

#[test]
fn memory_lock_factory_rejects_empty_lock_names() {
    let factory = MemoryLockFactory::new();

    assert!(matches!(
        factory.obtain_lock(""),
        Err(LockError::InvalidEmptyName)
    ));
}
