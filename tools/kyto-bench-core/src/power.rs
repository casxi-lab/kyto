//! The rules of the board's supply (`docs/bench/formats.md`, checks of a
//! cold boot): when readings confirm a power command, and whether a cold
//! boot keeps the order of its events and the off interval of the board
//! description. A run decides with [`Confirming`] when a command is
//! confirmed, and a run and a reader of its receipt check the recorded
//! events with the same functions.

use crate::board::{Board, Step};
use crate::instance::{Confirmation, Quantity};
use crate::receipt::{Check, Command, Event, EventKind, Outcome};

/// The confirmation of one power command by the readings that follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Confirming {
    command: Command,
    sent: u64,
    deadline: u64,
    run: u32,
}

impl Confirming {
    /// Waits for the confirmation of `command`, sent at `at_ns`, within the
    /// timeout of `confirmation` for its state.
    pub fn new(confirmation: &Confirmation, command: Command, at_ns: u64) -> Self {
        let timeout = match command {
            Command::On => confirmation.on_timeout_ms,
            Command::Off => confirmation.off_timeout_ms,
        };
        let (sent, deadline, run) = (at_ns, at_ns.saturating_add(nanos(timeout)), 0);
        Self {
            command,
            sent,
            deadline,
            run,
        }
    }

    /// The time by which a reading must confirm the command.
    pub fn deadline(&self) -> u64 {
        self.deadline
    }

    /// Takes the next reading, of `quantity`, `micro` at `at_ns`, and
    /// returns whether it confirms the command: it is the `consecutive`-th
    /// reading in a row of the configured quantity at or beyond the
    /// threshold of the command's state, all taken from the time the command
    /// was sent to the deadline.
    pub fn read(
        &mut self,
        confirmation: &Confirmation,
        quantity: Quantity,
        micro: i64,
        at_ns: u64,
    ) -> bool {
        let beyond = match self.command {
            Command::On => micro >= confirmation.on_at_least_micro,
            Command::Off => micro <= confirmation.off_at_most_micro,
        };
        let within = (self.sent..=self.deadline).contains(&at_ns);
        let counts = quantity == confirmation.quantity && beyond && within;
        self.run = if counts { self.run + 1 } else { 0 };
        self.run >= confirmation.consecutive
    }
}

/// The power command of `event`, if it is one.
pub fn command(event: &Event) -> Option<Command> {
    match event.event {
        EventKind::Power { command, .. } => Some(command),
        _ => None,
    }
}

/// The time of the reading that confirms the power command `events[index]`:
/// the first that [`Confirming::read`] takes as confirming among the readings
/// up to the next power command; `None` when none does or the event is not a
/// power command.
pub fn confirmed(confirmation: &Confirmation, events: &[Event], index: usize) -> Option<u64> {
    let state = command(events.get(index)?)?;
    let mut confirming = Confirming::new(confirmation, state, events[index].at_ns);
    let mut after = events[index + 1..]
        .iter()
        .take_while(|event| command(event).is_none());
    after.find_map(|event| match event.event {
        EventKind::Reading { quantity, micro } => {
            let confirms = confirming.read(confirmation, quantity, micro, event.at_ns);
            confirms.then_some(event.at_ns)
        }
        _ => None,
    })
}

/// When the supply was last confirmed off in `events`: the time of the
/// confirmation of their last power command, when it is an off and
/// confirmed. The board may be on otherwise, once a command was sent.
pub fn off_confirmed_at(confirmation: &Confirmation, events: &[Event]) -> Option<u64> {
    let last = events.iter().rposition(|event| command(event).is_some())?;
    let off = command(&events[last]) == Some(Command::Off);
    off.then(|| confirmed(confirmation, events, last)).flatten()
}

/// The check `power.off_unconfirmed` of the power off that `events`, those
/// before the first entry, end with, on the subject `start`.
pub fn start_check(confirmation: &Confirmation, events: &[Event]) -> Check {
    let missing = off_confirmed_at(confirmation, events).is_none();
    let failure = missing.then(|| unconfirmed("off", confirmation.off_timeout_ms));
    fault("power.off_unconfirmed", "start", failure)
}

/// The checks of the supply, the console and the devices in a cold boot
/// whose events are `events`, the supply having been confirmed off before
/// it at `off_at`, each a fault when it does not pass, in this order:
/// `console.not_ready`, `power.on_unconfirmed`, `power.off_interval` when
/// the supply was switched on, `power.off_unconfirmed`, `power.order` and
/// [`device_errors`].
pub fn cold_boot_checks(
    board: &Board,
    confirmation: &Confirmation,
    events: &[Event],
    off_at: Option<u64>,
) -> Vec<Check> {
    let ready = events
        .iter()
        .position(|event| matches!(event.event, EventKind::ConsoleReady {}));
    let on = events
        .iter()
        .position(|event| command(event) == Some(Command::On));
    let not_ready = ready.is_none().then(|| "the console was not ready".into());
    let mut checks = vec![fault("console.not_ready", "console", not_ready)];
    let on_missing = match on {
        None => Some("the supply was not switched on".into()),
        Some(on) => {
            let missing = confirmed(confirmation, events, on).is_none();
            missing.then(|| unconfirmed("on", confirmation.on_timeout_ms))
        }
    };
    checks.push(fault("power.on_unconfirmed", "on", on_missing));
    if let Some(on) = on {
        // Every step after the wait in the board's order starts once the
        // interval has passed since the supply was confirmed off.
        let (order, interval) = (&board.cold_boot.order, board.cold_boot.off_interval_ms);
        let after = order
            .iter()
            .skip_while(|step| **step != Step::WaitOffInterval);
        let started = after.skip(1).filter_map(|step| match step {
            Step::OpenConsole => ready,
            Step::PowerOn => Some(on),
            _ => None,
        });
        let until = off_at.map(|off| off.saturating_add(nanos(interval)));
        let early = started
            .into_iter()
            .any(|at| until.is_none_or(|until| events[at].at_ns < until));
        let failure = early.then(|| format!("a step began within {interval} ms of the off"));
        checks.push(fault("power.off_interval", "on", failure));
    }
    let off_missing = off_confirmed_at(confirmation, events).is_none();
    let failure = off_missing.then(|| unconfirmed("off", confirmation.off_timeout_ms));
    checks.push(fault("power.off_unconfirmed", "off", failure));
    let disorder = disorder(events).map(|index| format!("event {index} is out of order"));
    checks.push(fault("power.order", "events", disorder));
    checks.extend(device_errors(events));
    checks
}

/// The checks `device.error` of `events`: one fault for each failure of a
/// device, on the subject of its role.
pub fn device_errors(events: &[Event]) -> impl Iterator<Item = Check> + '_ {
    events.iter().filter_map(|event| match &event.event {
        EventKind::DeviceError { role, detail } => {
            Some(fault("device.error", role.name(), Some(detail.clone())))
        }
        _ => None,
    })
}

/// The first event of a cold boot that is out of order: its time or the
/// bytes received before it are less than those of the event before, or it
/// comes before an event of an earlier phase. The phases are, in order: the
/// console ready; the power on; the readings after it; the sends; the end;
/// the power off and the readings after it; the console closed.
fn disorder(events: &[Event]) -> Option<usize> {
    let (mut phase, mut last, mut state) = (0, (0, 0), None);
    events.iter().position(|event| {
        let next = match (&event.event, command(event).or(state)) {
            (EventKind::ConsoleReady {}, _) => 0,
            (EventKind::Power { .. } | EventKind::Reading { .. }, Some(Command::On)) => 1,
            (EventKind::Send { .. }, _) => 2,
            (EventKind::End { .. }, _) => 3,
            (EventKind::Power { .. } | EventKind::Reading { .. }, _) => 4,
            (EventKind::ConsoleClosed {}, _) => 5,
            _ => phase,
        };
        state = command(event).or(state);
        let out = next < phase || event.at_ns < last.0 || event.received < last.1;
        (phase, last) = (next, (event.at_ns, event.received));
        out
    })
}

fn fault(rule: &str, subject: &str, failure: Option<String>) -> Check {
    Check::new(rule, subject, Outcome::Fault, failure)
}

fn unconfirmed(state: &str, timeout_ms: u64) -> String {
    format!("no readings confirmed the power {state} within {timeout_ms} ms")
}

/// `ms` milliseconds in nanoseconds, or the most when that does not fit.
pub(crate) fn nanos(ms: u64) -> u64 {
    ms.saturating_mul(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::RASPBERRY_PI_5;
    use crate::instance::Role;
    use crate::receipt::tests::example;

    const MS: u64 = 1_000_000;

    fn confirmation(consecutive: u32) -> Confirmation {
        let mut confirmation = example().inputs.instance.parameters.confirmation;
        confirmation.consecutive = consecutive;
        confirmation
    }

    fn event(at_ms: u64, event: EventKind) -> Event {
        let (at_ns, received) = (at_ms * MS, 0);
        Event {
            at_ns,
            received,
            event,
        }
    }

    fn power(command: Command) -> EventKind {
        EventKind::Power {
            command,
            relay: None,
        }
    }

    fn volts(micro: i64) -> EventKind {
        let quantity = Quantity::SupplyVoltage;
        EventKind::Reading { quantity, micro }
    }

    /// The events of the cold boot of the worked example.
    fn worked() -> Vec<Event> {
        example().entries[0].cold_boots[0].events.clone()
    }

    fn results(checks: &[Check]) -> Vec<(&str, &str, Outcome)> {
        checks
            .iter()
            .map(|c| (c.rule.as_str(), c.subject.as_str(), c.result))
            .collect()
    }

    fn checks(events: &[Event], off_at: Option<u64>) -> Vec<Check> {
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        cold_boot_checks(&board, &confirmation(1), events, off_at)
    }

    /// The result of `rule` on `subject` among `checks`.
    fn result(checks: &[Check], rule: &str, subject: &str) -> Outcome {
        let found = checks
            .iter()
            .find(|c| c.rule == rule && c.subject == subject);
        found.unwrap().result
    }

    #[test]
    fn readings_in_a_row_beyond_the_threshold_confirm_a_command_by_its_timeout() {
        let conf = confirmation(2);
        let mut on = Confirming::new(&conf, Command::On, 1_000 * MS);
        assert_eq!(on.deadline(), 4_000 * MS);
        let volts = Quantity::SupplyVoltage;
        assert!(!on.read(&conf, volts, 4_750_000, 1_100 * MS));
        // A reading below the threshold starts the count again.
        assert!(!on.read(&conf, volts, 4_749_999, 1_200 * MS));
        assert!(!on.read(&conf, volts, 5_000_000, 1_300 * MS));
        assert!(on.read(&conf, volts, 5_000_000, 1_400 * MS));
        // A reading of another quantity or after the deadline does not count.
        let mut late = Confirming::new(&conf, Command::On, 0);
        assert!(!late.read(&conf, volts, 5_000_000, 2_000 * MS));
        assert!(!late.read(&conf, Quantity::SupplyCurrent, 5_000_000, 2_100 * MS));
        assert!(!late.read(&conf, volts, 5_000_000, 2_200 * MS));
        assert!(!late.read(&conf, volts, 5_000_000, 3_000 * MS + 1));
        // The off threshold is a reading at or below it.
        let one = confirmation(1);
        let mut off = Confirming::new(&one, Command::Off, 0);
        assert!(!off.read(&one, volts, 500_001, MS));
        assert!(off.read(&one, volts, 500_000, 2 * MS));
        // A reading taken before the command, such as one that a meter kept
        // from before, does not count.
        let mut stale = Confirming::new(&one, Command::Off, 1_000 * MS);
        assert!(!stale.read(&one, volts, 0, 999 * MS));
        assert!(stale.read(&one, volts, 0, 1_000 * MS));
    }

    #[test]
    fn a_reading_at_a_threshold_or_at_the_deadline_confirms_its_command() {
        let mut conf = confirmation(1);
        conf.off_timeout_ms = 2_000;
        let volts = Quantity::SupplyVoltage;
        let mut on = Confirming::new(&conf, Command::On, 1_000 * MS);
        assert!(on.read(&conf, volts, conf.on_at_least_micro, 4_000 * MS));
        // Each state has the timeout of its own.
        let mut late = Confirming::new(&conf, Command::Off, 1_000 * MS);
        assert_eq!(late.deadline(), 3_000 * MS);
        let (micro, mut last) = (conf.off_at_most_micro, late);
        assert!(!late.read(&conf, volts, micro, 3_000 * MS + 1));
        assert!(last.read(&conf, volts, micro, 3_000 * MS));
    }

    #[test]
    fn a_command_is_confirmed_by_the_readings_before_the_next_command() {
        let conf = confirmation(1);
        let events = [
            event(0, power(Command::Off)),
            event(100, volts(5_000_000)),
            event(200, power(Command::On)),
            event(300, volts(0)),
            event(400, volts(5_000_000)),
            event(500, power(Command::Off)),
        ];
        assert_eq!(confirmed(&conf, &events, 0), None);
        assert_eq!(confirmed(&conf, &events, 1), None);
        assert_eq!(confirmed(&conf, &events, 2), Some(400 * MS));
        assert_eq!(off_confirmed_at(&conf, &events), None);
        let mut off = events.to_vec();
        off.push(event(600, volts(10)));
        assert_eq!(off_confirmed_at(&conf, &off), Some(600 * MS));
        assert_eq!(off_confirmed_at(&conf, &events[..5]), None);
        assert_eq!(off_confirmed_at(&conf, &[]), None);
        let start = start_check(&conf, &off);
        assert_eq!(
            (start.result, start.subject.as_str()),
            (Outcome::Pass, "start")
        );
        assert_eq!(start_check(&conf, &events).result, Outcome::Fault);
        assert_eq!(start_check(&conf, &[]).result, Outcome::Fault);
        // A reading stamped before the power off confirms nothing.
        let stale = [event(1_000, power(Command::Off)), event(900, volts(0))];
        let start = start_check(&conf, &stale);
        let found = (start.rule.as_str(), start.subject.as_str(), start.result);
        assert_eq!(found, ("power.off_unconfirmed", "start", Outcome::Fault));
    }

    #[test]
    fn the_worked_example_keeps_every_rule_of_the_supply() {
        let checks = checks(&worked(), Some(100 * MS));
        let pass = |rule, subject| (rule, subject, Outcome::Pass);
        let expected = [
            pass("console.not_ready", "console"),
            pass("power.on_unconfirmed", "on"),
            pass("power.off_interval", "on"),
            pass("power.off_unconfirmed", "off"),
            pass("power.order", "events"),
        ];
        assert_eq!(results(&checks), expected);
    }

    #[test]
    fn an_unconfirmed_command_or_a_console_not_ready_is_a_fault() {
        let mut events = worked();
        events.remove(2);
        assert_eq!(
            result(&checks(&events, Some(0)), "power.on_unconfirmed", "on"),
            Outcome::Fault
        );
        let mut events = worked();
        events.remove(6);
        assert_eq!(
            result(&checks(&events, Some(0)), "power.off_unconfirmed", "off"),
            Outcome::Fault
        );
        // A cold boot that never switched the supply on or off.
        let events = [event(5_100, EventKind::ConsoleReady {})];
        let found = checks(&events, Some(0));
        assert_eq!(result(&found, "power.on_unconfirmed", "on"), Outcome::Fault);
        assert_eq!(
            result(&found, "power.off_unconfirmed", "off"),
            Outcome::Fault
        );
        assert!(!found.iter().any(|c| c.rule == "power.off_interval"));
        let mut events = worked();
        events.remove(0);
        let found = checks(&events, Some(0));
        assert_eq!(
            result(&found, "console.not_ready", "console"),
            Outcome::Fault
        );
        assert_eq!(
            found[0].detail.as_deref(),
            Some("the console was not ready")
        );
    }

    #[test]
    fn steps_after_the_wait_begin_once_the_off_interval_has_passed() {
        let events = worked();
        // The console is ready and the supply on at 5.1 s.
        assert_eq!(
            result(&checks(&events, Some(100 * MS)), "power.off_interval", "on"),
            Outcome::Pass
        );
        assert_eq!(
            result(
                &checks(&events, Some(100 * MS + 1)),
                "power.off_interval",
                "on"
            ),
            Outcome::Fault
        );
        assert_eq!(
            result(&checks(&events, None), "power.off_interval", "on"),
            Outcome::Fault
        );
        // With the console opened before the wait, only the power on counts.
        let order = "order = [\"publish\", \"wait_off_interval\", \"open_console\", \"power_on\"]";
        let text = RASPBERRY_PI_5.replace(
            order,
            "order = [\"open_console\", \"publish\", \"wait_off_interval\", \"power_on\"]",
        );
        let board = Board::parse(&text).unwrap();
        let mut events = worked();
        events[0].at_ns = 0;
        let found = cold_boot_checks(&board, &confirmation(1), &events, Some(100 * MS));
        assert_eq!(result(&found, "power.off_interval", "on"), Outcome::Pass);
        // With the console opened right after a wait that comes first, it
        // counts as well.
        let text = RASPBERRY_PI_5.replace(
            order,
            "order = [\"wait_off_interval\", \"open_console\", \"publish\", \"power_on\"]",
        );
        let board = Board::parse(&text).unwrap();
        let found = cold_boot_checks(&board, &confirmation(1), &events, Some(100 * MS));
        assert_eq!(result(&found, "power.off_interval", "on"), Outcome::Fault);
        assert_eq!(
            result(&checks(&events, Some(100 * MS)), "power.off_interval", "on"),
            Outcome::Fault
        );
    }

    #[test]
    fn events_out_of_order_are_a_fault() {
        let order = |events: &[Event]| result(&checks(events, Some(0)), "power.order", "events");
        assert_eq!(order(&worked()), Outcome::Pass);
        // A send before the power on, readings before it, a second power
        // on, an earlier time and fewer bytes received than before.
        let mut send = worked();
        send.swap(1, 3);
        let mut early = worked();
        early.insert(0, event(0, volts(0)));
        let mut again = worked();
        again.insert(7, event(7_800, power(Command::On)));
        let mut time = worked();
        time[4].at_ns = time[3].at_ns - 1;
        let mut bytes = worked();
        bytes[5].received -= 1;
        let mut closed = worked();
        closed.swap(5, 7);
        for events in [send, early, again, time, bytes, closed] {
            assert_eq!(order(&events), Outcome::Fault, "{events:?}");
        }
        let disorder = checks(
            &{
                let mut events = worked();
                events.swap(3, 4);
                events
            },
            Some(0),
        );
        let found = disorder.iter().find(|c| c.rule == "power.order").unwrap();
        assert_eq!(found.detail.as_deref(), Some("event 4 is out of order"));
    }

    #[test]
    fn every_device_error_is_a_fault() {
        let mut events = worked();
        let failed = |at_ms, role, detail: &str| {
            let detail = detail.to_string();
            let failed = event(at_ms, EventKind::DeviceError { role, detail });
            Event {
                received: 61,
                ..failed
            }
        };
        events.insert(5, failed(7_700, Role::Console, "the port closed"));
        events.push(failed(7_800, Role::Power, "no answer"));
        let found = checks(&events, Some(0));
        let errors: Vec<_> = found.iter().filter(|c| c.rule == "device.error").collect();
        assert_eq!(errors.len(), 2);
        assert_eq!(
            (errors[0].subject.as_str(), errors[0].result),
            ("console", Outcome::Fault)
        );
        assert_eq!(errors[1].detail.as_deref(), Some("no answer"));
        assert_eq!(result(&found, "power.order", "events"), Outcome::Pass);
    }
}
