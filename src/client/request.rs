use crate::auth::AuthManager;

pub enum Request {
    AuthDecode {
        url: String,
        auth_manager: AuthManager,
    },
    Get {
        url: String,
    },
    GetAuth {
        url: String,
        auth_manager: AuthManager,
    },
}
