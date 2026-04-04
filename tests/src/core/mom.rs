use std::any::Any;

use machina_core::mobject::{MObject, MObjectState};

struct TestObject {
    state: MObjectState,
}

impl TestObject {
    fn new_root(local_id: &str) -> Self {
        Self {
            state: MObjectState::new_root(local_id).expect("valid root object"),
        }
    }

    fn new_detached(local_id: &str) -> Self {
        Self {
            state: MObjectState::new_detached(local_id)
                .expect("valid detached object"),
        }
    }
}

impl MObject for TestObject {
    fn mobject_state(&self) -> &MObjectState {
        &self.state
    }

    fn mobject_state_mut(&mut self) -> &mut MObjectState {
        &mut self.state
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[test]
fn test_mobject_root_path() {
    let root = TestObject::new_root("machine");
    assert_eq!(root.local_id(), "machine");
    assert_eq!(root.object_path(), Some("/machine"));
    assert!(root.parent_path().is_none());
    assert!(root.child_paths().is_empty());
}

#[test]
fn test_mobject_attach_child_sets_parent_and_path() {
    let mut root = TestObject::new_root("machine");
    let mut child = TestObject::new_detached("uart0");

    root.mobject_state_mut()
        .attach_child(child.mobject_state_mut())
        .expect("attach child");

    assert_eq!(child.parent_path(), Some("/machine"));
    assert_eq!(child.object_path(), Some("/machine/uart0"));
    assert_eq!(root.child_paths(), &["/machine/uart0".to_string()]);
}

#[test]
fn test_mobject_duplicate_child_id_rejected() {
    let mut root = TestObject::new_root("machine");
    let mut child1 = TestObject::new_detached("uart0");
    let mut child2 = TestObject::new_detached("uart0");

    root.mobject_state_mut()
        .attach_child(child1.mobject_state_mut())
        .expect("attach first child");
    let err = root
        .mobject_state_mut()
        .attach_child(child2.mobject_state_mut())
        .expect_err("duplicate child id must fail");
    assert_eq!(
        err.to_string(),
        "child local_id 'uart0' is already attached under parent"
    );
}

#[test]
fn test_mobject_detach_child_clears_attachment() {
    let mut root = TestObject::new_root("machine");
    let mut child = TestObject::new_detached("uart0");

    root.mobject_state_mut()
        .attach_child(child.mobject_state_mut())
        .expect("attach child");
    root.mobject_state_mut()
        .detach_child(child.mobject_state_mut())
        .expect("detach child");

    assert!(root.child_paths().is_empty());
    assert!(child.parent_path().is_none());
    assert!(child.object_path().is_none());
}

#[test]
fn test_mobject_compile_time_type_check() {
    let obj = TestObject::new_root("machine");
    assert!(obj.is_type::<TestObject>());
}
