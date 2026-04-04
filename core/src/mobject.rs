use std::any::Any;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MObjectError {
    EmptyLocalId,
    ParentDetached,
    ChildAlreadyAttached,
    DuplicateChildId(String),
    ChildPathMismatch,
}

impl fmt::Display for MObjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLocalId => {
                write!(f, "mobject local_id must not be empty")
            }
            Self::ParentDetached => {
                write!(
                    f,
                    "parent mobject must have an object path before attachment"
                )
            }
            Self::ChildAlreadyAttached => {
                write!(f, "child mobject is already attached to a parent")
            }
            Self::DuplicateChildId(id) => {
                write!(
                    f,
                    "child local_id '{id}' is already attached under parent"
                )
            }
            Self::ChildPathMismatch => {
                write!(f, "child path does not match parent attachment state")
            }
        }
    }
}

impl std::error::Error for MObjectError {}

#[derive(Debug, Clone)]
pub struct MObjectState {
    local_id: String,
    object_path: Option<String>,
    parent_path: Option<String>,
    child_paths: Vec<String>,
}

impl MObjectState {
    pub fn new_root(local_id: &str) -> Result<Self, MObjectError> {
        validate_local_id(local_id)?;
        Ok(Self {
            local_id: local_id.to_string(),
            object_path: Some(root_path(local_id)),
            parent_path: None,
            child_paths: Vec::new(),
        })
    }

    pub fn new_detached(local_id: &str) -> Result<Self, MObjectError> {
        validate_local_id(local_id)?;
        Ok(Self {
            local_id: local_id.to_string(),
            object_path: None,
            parent_path: None,
            child_paths: Vec::new(),
        })
    }

    pub fn local_id(&self) -> &str {
        &self.local_id
    }

    pub fn object_path(&self) -> Option<&str> {
        self.object_path.as_deref()
    }

    pub fn parent_path(&self) -> Option<&str> {
        self.parent_path.as_deref()
    }

    pub fn child_paths(&self) -> &[String] {
        &self.child_paths
    }

    pub fn is_root(&self) -> bool {
        self.object_path.is_some() && self.parent_path.is_none()
    }

    pub fn is_attached(&self) -> bool {
        self.object_path.is_some()
    }

    pub fn attach_child(
        &mut self,
        child: &mut MObjectState,
    ) -> Result<(), MObjectError> {
        let parent_path = self
            .object_path
            .clone()
            .ok_or(MObjectError::ParentDetached)?;
        if child.parent_path.is_some() || child.object_path.is_some() {
            return Err(MObjectError::ChildAlreadyAttached);
        }

        let child_path = child_path(&parent_path, &child.local_id);
        if self.child_paths.iter().any(|path| path == &child_path) {
            return Err(MObjectError::DuplicateChildId(child.local_id.clone()));
        }

        child.parent_path = Some(parent_path);
        child.object_path = Some(child_path.clone());
        self.child_paths.push(child_path);
        Ok(())
    }

    pub fn detach_child(
        &mut self,
        child: &mut MObjectState,
    ) -> Result<(), MObjectError> {
        let Some(child_path) = child.object_path.clone() else {
            return Err(MObjectError::ChildPathMismatch);
        };
        if !self.child_paths.iter().any(|path| path == &child_path) {
            return Err(MObjectError::ChildPathMismatch);
        }

        self.child_paths.retain(|path| path != &child_path);
        child.parent_path = None;
        child.object_path = None;
        Ok(())
    }
}

pub trait MObject: Any + Send + Sync {
    fn mobject_state(&self) -> &MObjectState;
    fn mobject_state_mut(&mut self) -> &mut MObjectState;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn local_id(&self) -> &str {
        self.mobject_state().local_id()
    }

    fn object_path(&self) -> Option<&str> {
        self.mobject_state().object_path()
    }

    fn parent_path(&self) -> Option<&str> {
        self.mobject_state().parent_path()
    }

    fn child_paths(&self) -> &[String] {
        self.mobject_state().child_paths()
    }

    fn is_type<T: Any>(&self) -> bool {
        self.as_any().is::<T>()
    }
}

fn validate_local_id(local_id: &str) -> Result<(), MObjectError> {
    if local_id.is_empty() {
        return Err(MObjectError::EmptyLocalId);
    }
    Ok(())
}

fn root_path(local_id: &str) -> String {
    format!("/{local_id}")
}

fn child_path(parent_path: &str, child_local_id: &str) -> String {
    format!("{parent_path}/{child_local_id}")
}
