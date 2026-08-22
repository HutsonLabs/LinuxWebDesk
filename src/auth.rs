//! Authentication against the host's PAM stack.
//!
//! Terminating at PAM rather than binding directly to a directory is what makes
//! the rest of this program simple: a successful login yields a real uid, gid
//! and home directory, so the helper can become that user and the kernel can do
//! the access control. Whether the account lives in /etc/passwd, LDAP via SSSD,
//! or Kerberos is then somebody else's configuration problem, not ours.

pub struct Identity {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
}

/// The PAM service name. Install a matching file at /etc/pam.d/rockywebde.
pub const PAM_SERVICE: &str = "rockywebde";

#[cfg(target_os = "linux")]
pub fn authenticate(username: &str, password: &str) -> Result<Identity, String> {
    use pam_client::conv_mock::Conversation;
    use pam_client::{Context, Flag};

    let mut ctx = Context::new(
        PAM_SERVICE,
        Some(username),
        Conversation::with_credentials(username, password),
    )
    .map_err(|e| format!("pam init failed: {e}"))?;

    // authenticate() proves the credential. acct_mgmt() is the check people
    // forget: it enforces expired passwords, locked and disabled accounts, and
    // any access restrictions the PAM stack imposes.
    ctx.authenticate(Flag::NONE).map_err(|_| "invalid username or password".to_string())?;
    ctx.acct_mgmt(Flag::NONE).map_err(|e| format!("account unavailable: {e}"))?;

    resolve(username)
}

#[cfg(not(target_os = "linux"))]
pub fn authenticate(_username: &str, _password: &str) -> Result<Identity, String> {
    Err("PAM authentication is only supported on Linux".into())
}

/// Look the account up in NSS, so LDAP/SSSD-backed users resolve the same way
/// local ones do.
pub fn resolve(username: &str) -> Result<Identity, String> {
    use users::os::unix::UserExt;

    let user = users::get_user_by_name(username)
        .ok_or_else(|| format!("no such account: {username}"))?;

    let uid = user.uid();
    if uid == 0 {
        return Err("refusing to open a session for root".into());
    }

    let home = user
        .home_dir()
        .to_str()
        .ok_or("home directory is not valid UTF-8")?
        .to_string();

    Ok(Identity {
        username: user.name().to_string_lossy().to_string(),
        uid,
        gid: user.primary_group_id(),
        home,
    })
}
