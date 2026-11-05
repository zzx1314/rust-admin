/// Row level data scope for business records that belong to a user.
///
/// `owner_filter` returns `None` for administrators (no filtering) and
/// `Some(user_id)` for regular users (only their own rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataScope {
    user_id: i64,
    is_admin: bool,
}

impl DataScope {
    pub fn new(user_id: i64, is_admin: bool) -> Self {
        Self { user_id, is_admin }
    }

    pub fn user_id(&self) -> i64 {
        self.user_id
    }

    pub fn is_admin(&self) -> bool {
        self.is_admin
    }

    pub fn owner_filter(&self) -> Option<i64> {
        if self.is_admin {
            None
        } else {
            Some(self.user_id)
        }
    }
}
