//! Authentication against the host's PAM stack.
//!
//! Terminating at PAM rather than binding directly to a directory is what makes
//! the rest of this program simple: a successful login yields a real uid, gid
//! and home directory, so the helper can become that user and the kernel can do
//! the access control. Whether the account lives in /etc/passwd, LDAP via SSSD,
//! or Kerberos is then the host's configuration problem, not ours.
//!
//! The FFI below is written by hand rather than generated. It is a handful of
//! stable functions, and binding them directly keeps `bindgen` and `libclang`
//! out of the build -- which matters when the thing has to compile on both
//! Debian and RHEL without dragging in a compiler toolchain to do it.

pub struct Identity {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
}

/// The PAM service name. `install.sh` writes a matching /etc/pam.d/linuxwebdesk.
pub const PAM_SERVICE: &str = "linuxwebdesk";

#[cfg(target_os = "linux")]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    pub const PAM_SUCCESS: c_int = 0;
    pub const PAM_CONV_ERR: c_int = 19;
    pub const PAM_PROMPT_ECHO_OFF: c_int = 1;
    pub const PAM_PROMPT_ECHO_ON: c_int = 2;

    #[repr(C)]
    pub struct PamMessage {
        pub msg_style: c_int,
        pub msg: *const c_char,
    }

    #[repr(C)]
    pub struct PamResponse {
        pub resp: *mut c_char,
        pub resp_retcode: c_int,
    }

    pub type ConvFn = unsafe extern "C" fn(
        num_msg: c_int,
        msg: *const *const PamMessage,
        resp: *mut *mut PamResponse,
        appdata: *mut c_void,
    ) -> c_int;

    #[repr(C)]
    pub struct PamConv {
        pub conv: Option<ConvFn>,
        pub appdata_ptr: *mut c_void,
    }

    pub enum PamHandle {}

    extern "C" {
        pub fn pam_start(
            service: *const c_char,
            user: *const c_char,
            conv: *const PamConv,
            handle: *mut *mut PamHandle,
        ) -> c_int;
        pub fn pam_authenticate(handle: *mut PamHandle, flags: c_int) -> c_int;
        pub fn pam_acct_mgmt(handle: *mut PamHandle, flags: c_int) -> c_int;
        pub fn pam_end(handle: *mut PamHandle, status: c_int) -> c_int;
        pub fn pam_strerror(handle: *mut PamHandle, errnum: c_int) -> *const c_char;
    }
}

#[cfg(target_os = "linux")]
struct Creds {
    user: std::ffi::CString,
    pass: std::ffi::CString,
}

/// PAM calls this to ask for the password. It allocates the response array with
/// `calloc` and each reply with `strdup` because libpam takes ownership and
/// frees them itself.
#[cfg(target_os = "linux")]
unsafe extern "C" fn converse(
    num_msg: std::ffi::c_int,
    msg: *const *const ffi::PamMessage,
    resp: *mut *mut ffi::PamResponse,
    appdata: *mut std::ffi::c_void,
) -> std::ffi::c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata.is_null() {
        return ffi::PAM_CONV_ERR;
    }
    let creds = &*(appdata as *const Creds);
    let n = num_msg as usize;

    let arr = libc::calloc(n, std::mem::size_of::<ffi::PamResponse>()) as *mut ffi::PamResponse;
    if arr.is_null() {
        return ffi::PAM_CONV_ERR;
    }

    for i in 0..n {
        let m = *msg.add(i);
        let slot = arr.add(i);
        (*slot).resp_retcode = 0;
        (*slot).resp = std::ptr::null_mut();
        if m.is_null() {
            continue;
        }
        let reply = match (*m).msg_style {
            ffi::PAM_PROMPT_ECHO_OFF => creds.pass.as_ptr(),
            ffi::PAM_PROMPT_ECHO_ON => creds.user.as_ptr(),
            _ => continue, // PAM_TEXT_INFO / PAM_ERROR_MSG need no reply
        };
        (*slot).resp = libc::strdup(reply);
    }

    *resp = arr;
    ffi::PAM_SUCCESS
}

#[cfg(target_os = "linux")]
pub fn authenticate(username: &str, password: &str) -> Result<Identity, String> {
    use std::ffi::CString;

    // CString::new rejects interior NULs, so a crafted username cannot truncate
    // what PAM sees.
    let service = CString::new(PAM_SERVICE).map_err(|_| "bad service name")?;
    let cuser = CString::new(username).map_err(|_| "invalid username")?;
    let creds = Creds {
        user: CString::new(username).map_err(|_| "invalid username")?,
        pass: CString::new(password).map_err(|_| "invalid password")?,
    };

    let conv = ffi::PamConv {
        conv: Some(converse),
        appdata_ptr: &creds as *const Creds as *mut std::ffi::c_void,
    };

    let mut handle: *mut ffi::PamHandle = std::ptr::null_mut();
    let rc = unsafe { ffi::pam_start(service.as_ptr(), cuser.as_ptr(), &conv, &mut handle) };
    if rc != ffi::PAM_SUCCESS || handle.is_null() {
        return Err(format!("pam_start failed (code {rc})"));
    }

    let auth_rc = unsafe { ffi::pam_authenticate(handle, 0) };
    if auth_rc != ffi::PAM_SUCCESS {
        let why = strerror(handle, auth_rc);
        unsafe { ffi::pam_end(handle, auth_rc) };
        return Err(format!("authentication failed: {why}"));
    }

    // The check people forget: expired passwords, locked and disabled accounts,
    // and any access restrictions the PAM stack imposes.
    let acct_rc = unsafe { ffi::pam_acct_mgmt(handle, 0) };
    if acct_rc != ffi::PAM_SUCCESS {
        let why = strerror(handle, acct_rc);
        unsafe { ffi::pam_end(handle, acct_rc) };
        return Err(format!("account unavailable: {why}"));
    }

    unsafe { ffi::pam_end(handle, ffi::PAM_SUCCESS) };
    resolve(username)
}

#[cfg(target_os = "linux")]
fn strerror(handle: *mut ffi::PamHandle, code: std::ffi::c_int) -> String {
    unsafe {
        let p = ffi::pam_strerror(handle, code);
        if p.is_null() {
            return format!("code {code}");
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

#[cfg(not(target_os = "linux"))]
pub fn authenticate(_username: &str, _password: &str) -> Result<Identity, String> {
    Err("PAM authentication is only supported on Linux".into())
}

/// Look the account up through NSS, so SSSD- and LDAP-backed users resolve
/// exactly the way local ones do.
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
