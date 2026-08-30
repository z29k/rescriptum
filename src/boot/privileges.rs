//! Dropping privileges after binding, the way dnsmasq and nginx do it.
//!
//! TFTP wants port 69, which is privileged. That is the *only* privileged port this
//! server ever asks for — with no DHCP responder there is nothing wanting 67 or 4011 —
//! so the whole question is: bind one low port, then stop being root.
//!
//! **Bind every listener first, then drop, then say what the process now is.** Dropping
//! before binding is the bug that works in testing as root and fails on deployment, and
//! it fails at the one moment nobody is watching: a reboot.
//!
//! This costs **zero new crates**: `libc` is already in `Cargo.lock` by way of tokio, so
//! promoting it from transitive to direct adds nothing to the build.
//!
//! Two other answers exist and neither needs this code, so both are documented rather
//! than implemented: socket activation (`LISTEN_FDS`) on a systemd host, which needs no
//! privileges at all, and `setcap cap_net_bind_service` on the binary.

/// Become `user` and `group`, in the order that actually works.
///
/// The order is not a style choice. Supplementary groups must go **before** the primary
/// group, and the group before the user: `setuid` is what surrenders the privilege to
/// call the other two, so doing it first leaves a process that kept every group it had.
/// That is the classic privilege-dropping bug, and it is silent — the process looks
/// unprivileged and is not.
pub fn drop_to(user: Option<&str>, group: Option<&str>) -> Result<(), String> {
    if user.is_none() && group.is_none() {
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        let _ = (user, group);
        return Err(
            "RESCRIPTUM_USER and RESCRIPTUM_GROUP are Unix concepts, and this is not Unix"
                .to_string(),
        );
    }

    #[cfg(unix)]
    {
        // Nothing to drop *to*, and nothing to drop *from*: a non-root process cannot
        // change identity, and pretending otherwise would fail later and less clearly.
        let root = unsafe { libc::geteuid() } == 0;
        if !root {
            return Err(format!(
                "RESCRIPTUM_USER{} is set, but this process is not root, so it cannot change \
                 identity. Either start as root — binding port 69 needs it anyway — or drop \
                 the setting and give the binary `setcap cap_net_bind_service`.",
                match group {
                    Some(_) => "/RESCRIPTUM_GROUP",
                    None => "",
                }
            ));
        }

        let gid = match group {
            Some(name) => Some(lookup_group(name)?),
            None => None,
        };
        let target = match user {
            Some(name) => Some(lookup_user(name)?),
            None => None,
        };

        // The group first, and the supplementary set before that.
        // An explicit group wins; otherwise take the user's own primary group, which is
        // what naming only a user is asking for.
        let primary = gid.or(target.as_ref().map(|(_, gid, _)| *gid));
        if let Some(gid) = primary {
            // Supplementary groups survive `setgid` on their own, and a process that
            // kept root's is not unprivileged in any sense that matters.
            let name = user.map(std::ffi::CString::new).transpose().ok().flatten();
            let result = match &name {
                // `as _`, not a named type: Darwin declares the base group as `int` and
                // Linux as `gid_t`, so spelling either one out breaks the other.
                Some(name) => unsafe { libc::initgroups(name.as_ptr(), gid as _) },
                // No user named, so there is no supplementary set to compute. Clear it.
                None => unsafe { libc::setgroups(0, std::ptr::null()) },
            };
            let initialised = result == 0;
            if !initialised {
                return Err(format!(
                    "cannot set the supplementary groups for gid {gid}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if unsafe { libc::setgid(gid as libc::gid_t) } != 0 {
                return Err(format!(
                    "cannot become group {gid}: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        if let Some((uid, _, name)) = target {
            if unsafe { libc::setuid(uid as libc::uid_t) } != 0 {
                return Err(format!(
                    "cannot become user {name}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // **Verify rather than assume.** On some systems a failed drop returns
            // success; a process that thinks it dropped and did not is worse than one
            // that never tried, because nothing will ever check again.
            if unsafe { libc::setuid(0) } == 0 {
                return Err(
                    "dropped to an unprivileged user and was still able to become root again \
                     — refusing to run in that state"
                        .to_string(),
                );
            }
        }

        crate::log::server(&format!(
            "dropped privileges: uid={} gid={}",
            unsafe { libc::getuid() },
            unsafe { libc::getgid() }
        ));
        Ok(())
    }
}

#[cfg(unix)]
fn lookup_user(name: &str) -> Result<(u32, u32, String), String> {
    // A numeric value is an identity in its own right — a container image often has no
    // passwd entry at all, and refusing one there would be refusing the normal case.
    if let Ok(uid) = name.parse::<u32>() {
        return Ok((uid, uid, name.to_string()));
    }
    let c_name = std::ffi::CString::new(name).map_err(|_| format!("{name:?} is not a name"))?;
    let entry = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if entry.is_null() {
        return Err(format!(
            "RESCRIPTUM_USER={name:?} does not exist on this system"
        ));
    }
    let entry = unsafe { *entry };
    Ok((entry.pw_uid as u32, entry.pw_gid as u32, name.to_string()))
}

#[cfg(unix)]
fn lookup_group(name: &str) -> Result<u32, String> {
    if let Ok(gid) = name.parse::<u32>() {
        return Ok(gid);
    }
    let c_name = std::ffi::CString::new(name).map_err(|_| format!("{name:?} is not a name"))?;
    let entry = unsafe { libc::getgrnam(c_name.as_ptr()) };
    if entry.is_null() {
        return Err(format!(
            "RESCRIPTUM_GROUP={name:?} does not exist on this system"
        ));
    }
    Ok(unsafe { *entry }.gr_gid as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_nobody_does_nothing() {
        // The overwhelmingly common case: no setting, no syscall, no behaviour.
        assert!(drop_to(None, None).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn a_user_that_does_not_exist_is_refused_by_name() {
        // Whatever this process is, the answer must name the setting rather than fail
        // with an errno nobody can act on.
        let e = drop_to(Some("definitely-not-a-user-on-this-box"), None).expect_err("must refuse");
        assert!(
            e.contains("RESCRIPTUM_USER") || e.contains("not root"),
            "{e}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_non_root_process_says_so_rather_than_failing_obscurely() {
        // The test suite does not run as root, so this is the branch it can prove: the
        // error names the setting and offers the two alternatives.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let e = drop_to(Some("nobody"), None).expect_err("must refuse");
        assert!(e.contains("not root"), "{e}");
        assert!(e.contains("setcap"), "{e}");
    }

    #[test]
    #[cfg(unix)]
    fn a_numeric_identity_needs_no_passwd_entry() {
        // A container image often has no passwd file at all, and refusing a numeric uid
        // there would be refusing the normal case.
        assert_eq!(
            lookup_user("1000").expect("numeric"),
            (1000, 1000, "1000".to_string())
        );
        assert_eq!(lookup_group("1000").expect("numeric"), 1000);
    }

    #[test]
    #[cfg(unix)]
    fn a_real_account_resolves() {
        // `root` exists on every Unix this can run on, and looking it up proves the
        // getpwnam path rather than only the numeric shortcut.
        let (uid, _, name) = lookup_user("root").expect("root exists");
        assert_eq!(uid, 0);
        assert_eq!(name, "root");
    }
}
