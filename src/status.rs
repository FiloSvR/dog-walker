//! The status of a task: `created`, `running`, then exactly one end state (R12). Only
//! dog-walker writes it, and it only moves forward: a task that has ended never changes again.

use std::fmt;

use crate::error::StatusError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Created,
    Running,
    Done,
    Failed,
    TimedOut,
    Cancelled,
}

impl Status {
    /// Every status, in the order they appear in the enum.
    pub const ALL: [Status; 6] = [
        Status::Created,
        Status::Running,
        Status::Done,
        Status::Failed,
        Status::TimedOut,
        Status::Cancelled,
    ];

    /// The states a task can end in.
    pub const ENDS: [Status; 4] = [
        Status::Done,
        Status::Failed,
        Status::TimedOut,
        Status::Cancelled,
    ];

    /// The name used in the handover's front matter.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Created => "created",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::TimedOut => "timed-out",
            Status::Cancelled => "cancelled",
        }
    }

    /// Reads a status back from its name in a handover.
    pub fn parse(text: &str) -> Option<Status> {
        Status::ALL
            .into_iter()
            .find(|status| status.as_str() == text)
    }

    pub fn is_end(self) -> bool {
        Status::ENDS.contains(&self)
    }

    /// `created` to `running`, just before the harness starts.
    pub fn start(self) -> Result<Status, StatusError> {
        match self {
            Status::Created => Ok(Status::Running),
            from => Err(StatusError::Transition {
                from,
                to: Status::Running,
            }),
        }
    }

    /// `created` to `cancelled`: a task stopped while it was still waiting for a slot, so
    /// its harness never ran (R9, R12).
    pub fn cancel_before_start(self) -> Result<Status, StatusError> {
        match self {
            Status::Created => Ok(Status::Cancelled),
            from => Err(StatusError::Transition {
                from,
                to: Status::Cancelled,
            }),
        }
    }

    /// `running` to one of the end states, once the task has ended.
    pub fn end(self, to: Status) -> Result<Status, StatusError> {
        if !to.is_end() {
            return Err(StatusError::NotAnEnd { to });
        }
        match self {
            Status::Running => Ok(to),
            from => Err(StatusError::Transition { from, to }),
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_becomes_running_and_nothing_else_does() {
        assert_eq!(Status::Created.start().unwrap(), Status::Running);
        for from in Status::ENDS {
            assert!(from.start().is_err(), "{from}");
        }
        assert!(Status::Running.start().is_err());
    }

    #[test]
    fn a_created_task_can_be_cancelled_before_it_starts() {
        assert_eq!(
            Status::Created.cancel_before_start().unwrap(),
            Status::Cancelled
        );
        for from in Status::ENDS.into_iter().chain([Status::Running]) {
            assert!(from.cancel_before_start().is_err(), "{from}");
        }
    }

    #[test]
    fn only_a_running_task_can_end() {
        for to in Status::ENDS {
            assert_eq!(Status::Running.end(to).unwrap(), to);
            assert!(Status::Created.end(to).is_err());
            assert!(Status::Done.end(to).is_err());
        }
    }

    #[test]
    fn an_end_state_cannot_be_an_end_of_itself() {
        assert!(Status::Running.end(Status::Running).is_err());
        assert!(Status::Running.end(Status::Created).is_err());
    }

    #[test]
    fn names_are_the_handover_spellings() {
        let names: Vec<&str> = Status::ALL.iter().map(|status| status.as_str()).collect();
        assert_eq!(
            names,
            [
                "created",
                "running",
                "done",
                "failed",
                "timed-out",
                "cancelled"
            ]
        );
    }

    #[test]
    fn every_end_state_is_listed_in_all() {
        for end in Status::ENDS {
            assert!(Status::ALL.contains(&end), "{end}");
        }
    }
}
