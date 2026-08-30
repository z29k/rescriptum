//! Driving a controller: what `power on`, `power off`, `power pxe` and `power status` do,
//! for either kind of controller.
//!
//! Everything here is **synchronous and operator-triggered**. There is no reconciliation
//! loop, no agent deciding a machine "should" be reinstalled, and no retry: every action
//! is a person or a script, once. That is the line between this and a provisioning
//! platform, and it is the whole reason the feature is affordable.

use crate::controllers::{CommandHook, Controller, Kind};
use crate::redfish::{self, Client};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a machine is doing, as its controller reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    On,
    Off,
    /// A controller that presses buttons rather than reading firmware cannot say. A PDU
    /// knows whether it is feeding the outlet, not whether the machine booted.
    Unknown,
}

impl State {
    pub fn label(&self) -> &'static str {
        match self {
            State::On => "on",
            State::Off => "off",
            State::Unknown => "unknown",
        }
    }
}

/// One machine's state, plus whatever its controller could say about its next boot.
#[derive(Debug, Clone)]
pub struct Status {
    pub state: State,
    /// `Once`/`Pxe` when a one-time network boot is armed, as the *service* reports it
    /// rather than as we last asked for.
    pub boot_override: Option<String>,
}

/// Whether the one-time boot override actually took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Armed {
    /// The service confirms it on a read-back.
    Confirmed,
    /// The controller has no boot override at all, which is **not a failure**: the boot
    /// order stays on PXE and `RESCRIPTUM_BOOT_UNCLAIMED` plus the `installed-` disarm
    /// decide whether the machine installs. A PiKVM plus rescriptum is a complete
    /// solution; a BMC with one-time boot is belt and braces.
    NotSupported,
    /// The request succeeded and the state says otherwise. PiKVM's PATCH answers
    /// `204 No Content` and changes nothing, so this is a real outcome rather than a
    /// defensive branch.
    Ignored,
}

fn timeout_of(controller: &Controller) -> Duration {
    match &controller.kind {
        Kind::Redfish(_) => redfish::DEFAULT_TIMEOUT,
        Kind::Command(c) => c.timeout,
    }
}

pub fn status(controller: &Controller) -> Result<Status, String> {
    match &controller.kind {
        Kind::Command(_) => Ok(Status {
            // Not "off". A command controller drives an outlet or presses a button; it
            // has no way to know, and saying "off" would be an invention an operator
            // would act on.
            state: State::Unknown,
            boot_override: None,
        }),
        Kind::Redfish(r) => {
            let client = Client::new(r);
            let id = client.system_id().map_err(|e| e.to_string())?;
            let system = client.system(&id).map_err(|e| e.to_string())?;
            let state = match redfish::power_state(&system.body).as_deref() {
                Some("On") => State::On,
                Some("Off") => State::Off,
                _ => State::Unknown,
            };
            let (enabled, target) = redfish::boot_override(&system.body);
            let boot_override = match (enabled.as_deref(), target) {
                (Some("Disabled") | None, _) => None,
                (Some(e), Some(t)) => Some(format!("{e}/{t}")),
                (Some(e), None) => Some(e.to_string()),
            };
            Ok(Status {
                state,
                boot_override,
            })
        }
    }
}

pub fn on(controller: &Controller) -> Result<(), String> {
    match &controller.kind {
        Kind::Redfish(r) => reset(r, "On"),
        Kind::Command(c) => run(&c.on, c.timeout, "on"),
    }
}

/// `GracefulShutdown` unless `hard`, which is `ForceOff`.
///
/// The graceful form is the default because pulling power from a machine mid-write is how
/// a filesystem gets repaired by hand later; `--hard` is for the machine that has stopped
/// answering, which is the whole reason out-of-band control exists.
pub fn off(controller: &Controller, hard: bool) -> Result<(), String> {
    match &controller.kind {
        Kind::Redfish(r) => {
            if hard {
                reset(r, "ForceOff")
            } else {
                // Where a service offers no graceful form, `reset` names what it does
                // offer rather than reporting the 400 a wrong one earns.
                reset(r, "GracefulShutdown")
            }
        }
        Kind::Command(c) => run(&c.off, c.timeout, "off"),
    }
}

/// Restart, choosing the form the service actually offers.
///
/// `install` needs this because **`ResetType: "On"` sent to a system that is already on**
/// is refused by many implementations and treated as a no-op by others — either way
/// nothing happens while it looks like something did. A machine being reinstalled is
/// usually running, so this is the common case rather than the edge one.
pub fn restart(controller: &Controller) -> Result<(), String> {
    match &controller.kind {
        Kind::Redfish(r) => {
            let client = Client::new(r);
            let id = client.system_id().map_err(|e| e.to_string())?;
            let system = client.system(&id).map_err(|e| e.to_string())?;
            let allowed = redfish::allowable_resets(&system.body);
            // Graceful first, because a restart of a working machine should let it write
            // its filesystem out.
            let choice = ["GracefulRestart", "ForceRestart", "PowerCycle"]
                .into_iter()
                .find(|c| allowed.iter().any(|a| a == c))
                .ok_or_else(|| {
                    format!(
                        "this system offers no restart — it accepts {}",
                        allowed.join(", ")
                    )
                })?;
            client.reset(&id, choice).map_err(|e| e.to_string())
        }
        // A PDU has one way to restart something and it is not gentle. Off then on is
        // left to the operator rather than invented here, because the delay between them
        // is a property of the hardware.
        Kind::Command(_) => Err(
            "a command controller has no restart — power it off and on, with whatever \
             delay that hardware needs"
                .to_string(),
        ),
    }
}

/// Arm a one-time network boot, and say whether it actually took.
pub fn pxe(controller: &Controller) -> Result<Armed, String> {
    match &controller.kind {
        Kind::Redfish(r) => {
            let client = Client::new(r);
            let id = client.system_id().map_err(|e| e.to_string())?;
            match client.set_pxe_once(&id) {
                Ok(true) => Ok(Armed::Confirmed),
                Ok(false) => Ok(Armed::Ignored),
                Err(e) => Err(e.to_string()),
            }
        }
        Kind::Command(c) if c.pxe.is_empty() => Ok(Armed::NotSupported),
        Kind::Command(c) => run(&c.pxe, c.timeout, "pxe").map(|()| Armed::Confirmed),
    }
}

fn reset(r: &crate::controllers::Redfish, kind: &str) -> Result<(), String> {
    let client = Client::new(r);
    let id = client.system_id().map_err(|e| e.to_string())?;
    client.reset(&id, kind).map_err(|e| e.to_string())
}

/// Run one hook, with a deadline.
///
/// `std::process::Command` has no timeout of its own, so a hung `pdu` script would hang
/// `install` forever. Three rules, all of them load-bearing:
///
/// - **argv only, never a shell.** The vector comes from the controllers file as written
///   and is handed to `Command` unchanged: no `sh -c`, no splitting, no interpolation.
/// - **Nothing from a request reaches it.** Templating exists in this program and must not
///   arrive here; values in that file come from that file.
/// - **A deadline, and a killed child is an *unknown* outcome**, not a failure. A PDU
///   script that was killed halfway may well have switched the outlet.
///
/// stderr is inherited rather than captured, so the operator sees the script's own
/// complaint as it happens — and so that a chatty script cannot deadlock by filling a pipe
/// nobody is draining.
fn run(argv: &[String], timeout: Duration, what: &str) -> Result<(), String> {
    let Some((program, args)) = argv.split_first() else {
        return Err(format!("this controller has no `{what}` command"));
    };

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!(
                    "{program} exited {} — see its own output above",
                    status.code().unwrap_or(-1)
                ));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{program} did not finish within {}s and was killed — \
                         **the outcome is unknown**; check the hardware rather than \
                         running it again",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("{program}: {e}")),
        }
    }
}

/// How many controllers are asked at once by `power list --state`.
///
/// Bounded on purpose: two hundred controllers with a handful unreachable would otherwise
/// open two hundred connections and wait out every deadline at once. Small enough to be
/// kind to a NAS, large enough that a rack does not take minutes.
pub const PROBE_CONCURRENCY: usize = 8;

/// Ask several controllers at once, bounded, and give back what each said.
///
/// **Never on a redraw.** One unreachable BMC must not be able to freeze a screen, which
/// is why this is `--state` and not what a plain listing does.
pub fn probe(controllers: &[&Controller]) -> Vec<Result<Status, String>> {
    let mut out: Vec<Result<Status, String>> = Vec::with_capacity(controllers.len());
    for chunk in controllers.chunks(PROBE_CONCURRENCY) {
        let mut results: Vec<Result<Status, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|c| scope.spawn(move || status(c)))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        Err("the probe panicked, which is a bug — treat the state as \
                             unknown"
                            .to_string())
                    })
                })
                .collect()
        });
        out.append(&mut results);
    }
    out
}

/// The deadline one controller would take, for saying how long a listing might.
pub fn worst_case(controllers: &[&Controller]) -> Duration {
    controllers
        .iter()
        .map(|c| timeout_of(c))
        .max()
        .unwrap_or_default()
}

/// A hook's deadline, for a message.
pub fn hook_timeout(hook: &CommandHook) -> Duration {
    hook.timeout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers;

    /// Resolve a standard tool rather than hard-coding a path.
    ///
    /// `/bin/true` exists on Linux and **not on macOS**, where it is `/usr/bin/true`;
    /// `/bin/sleep` is the other way round on some distributions. A hard-coded path here
    /// is the "passed locally, failed in CI for a reason unrelated to the change" trap
    /// this repository has already been caught by once.
    fn tool(name: &str) -> String {
        ["/usr/bin", "/bin"]
            .iter()
            .map(|dir| format!("{dir}/{name}"))
            .find(|p| std::path::Path::new(p).is_file())
            .unwrap_or_else(|| panic!("no {name} on this system"))
    }

    fn command_controller(body: &str) -> Controller {
        controllers::parse(body)
            .expect("parse")
            .iter()
            .next()
            .expect("one")
            .clone()
    }

    #[test]
    fn a_command_controller_never_claims_to_know_the_power_state() {
        // A PDU knows whether it is feeding an outlet, not whether the machine booted.
        // Saying "off" would be an invention an operator would act on.
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("true")
        ));
        assert_eq!(status(&c).expect("status").state, State::Unknown);
    }

    #[test]
    fn a_hook_that_succeeds_is_a_success() {
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("true")
        ));
        assert!(on(&c).is_ok());
    }

    #[test]
    fn a_hook_that_fails_reports_its_exit_code() {
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("false")
        ));
        let e = on(&c).expect_err("must fail");
        assert!(e.contains("exited 1"), "{e}");
    }

    #[test]
    fn a_hook_that_does_not_exist_says_so_rather_than_panicking() {
        let c = command_controller("[\"aa\"]\nkind = \"command\"\non = [\"/nonexistent/pdu\"]\n");
        let e = on(&c).expect_err("must fail");
        assert!(e.contains("cannot run"), "{e}");
    }

    /// `std::process::Command` has no deadline of its own, so this is the whole reason
    /// `timeout` exists in the controllers file.
    #[test]
    fn a_hanging_hook_is_killed_and_reported_as_an_unknown_outcome() {
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\", \"30\"]\ntimeout = 1\n",
            tool("sleep")
        ));
        let started = Instant::now();
        let e = on(&c).expect_err("must time out");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it waited {:?}, so the deadline did not fire",
            started.elapsed()
        );
        assert!(e.contains("outcome is unknown"), "{e}");
        // And it must not suggest running it again: the outlet may already have switched.
        assert!(e.contains("rather than running it again"), "{e}");
    }

    #[test]
    fn a_controller_with_no_pxe_command_is_not_a_failure() {
        // Where one-time boot does not exist the boot order stays on PXE and the server
        // decides whether to install — which is a complete solution, not a gap.
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("true")
        ));
        assert_eq!(pxe(&c).expect("pxe"), Armed::NotSupported);
    }

    #[test]
    fn a_command_controller_has_no_restart_and_says_why() {
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("true")
        ));
        let e = restart(&c).expect_err("no restart");
        assert!(e.contains("off and on"), "{e}");
    }

    #[test]
    fn asking_for_a_command_the_controller_does_not_have_is_named() {
        let c = command_controller(&format!(
            "[\"aa\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("true")
        ));
        let e = off(&c, false).expect_err("no off command");
        assert!(e.contains("no `off` command"), "{e}");
    }

    #[test]
    fn probing_several_controllers_bounded_gives_one_answer_each() {
        let text = (0..20)
            .map(|n| {
                format!(
                    "[\"aa-bb-cc-dd-ee-{n:02}\"]\nkind = \"command\"\non = [\"{}\"]\n",
                    tool("true")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = controllers::parse(&text).expect("parse");
        let all: Vec<&Controller> = parsed.iter().collect();
        let results = probe(&all);
        assert_eq!(results.len(), 20);
        assert!(results.iter().all(Result::is_ok));
    }
}
