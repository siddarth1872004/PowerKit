//! Module lifecycle state machine.
//!
//! ```text
//! Created --Start--> Running <--Resume/Pause--> Paused
//!    |                  |                          |
//!    +------Fail--------+----------Fail------------+--> Failed --Start--> Running
//!                       +----------Stop------------+--> Stopped --Start--> Running
//! ```

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Constructed, never started.
    Created,
    Running,
    /// Started but temporarily inert (hotkeys released, hooks pass input through).
    Paused,
    Stopped,
    /// Start failed or the module panicked; it is inert until restarted.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Start,
    Pause,
    Resume,
    Stop,
    Fail,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("cannot {transition:?} a module that is {state:?}")]
pub struct InvalidTransition {
    pub state: State,
    pub transition: Transition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifecycle {
    state: State,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self { state: State::Created }
    }
}

impl Lifecycle {
    pub fn state(&self) -> State {
        self.state
    }

    /// Whether the module currently owns OS resources (hotkeys, hooks, windows).
    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Running | State::Paused)
    }

    pub fn next(state: State, transition: Transition) -> Result<State, InvalidTransition> {
        use State::*;
        use Transition::*;
        match (state, transition) {
            (Created | Stopped | Failed, Start) => Ok(Running),
            (Running, Pause) => Ok(Paused),
            (Paused, Resume) => Ok(Running),
            (Running | Paused, Stop) => Ok(Stopped),
            (_, Fail) => Ok(Failed),
            _ => Err(InvalidTransition { state, transition }),
        }
    }

    pub fn apply(&mut self, transition: Transition) -> Result<State, InvalidTransition> {
        self.state = Self::next(self.state, transition)?;
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn transition() -> impl Strategy<Value = Transition> {
        prop_oneof![
            Just(Transition::Start),
            Just(Transition::Pause),
            Just(Transition::Resume),
            Just(Transition::Stop),
            Just(Transition::Fail),
        ]
    }

    #[test]
    fn happy_path() {
        let mut l = Lifecycle::default();
        assert_eq!(l.apply(Transition::Start), Ok(State::Running));
        assert_eq!(l.apply(Transition::Pause), Ok(State::Paused));
        assert_eq!(l.apply(Transition::Resume), Ok(State::Running));
        assert_eq!(l.apply(Transition::Stop), Ok(State::Stopped));
        assert_eq!(l.apply(Transition::Start), Ok(State::Running));
    }

    #[test]
    fn rejects_invalid() {
        let mut l = Lifecycle::default();
        assert!(l.apply(Transition::Pause).is_err());
        assert!(l.apply(Transition::Stop).is_err());
        assert_eq!(l.state(), State::Created);
        l.apply(Transition::Start).unwrap();
        assert!(l.apply(Transition::Start).is_err());
        assert!(l.apply(Transition::Resume).is_err());
    }

    proptest! {
        /// Random transition sequences never leave the machine in an inconsistent state:
        /// a rejected transition does not change state, `Fail` always lands in `Failed`,
        /// and a module is only "active" after a successful start.
        #[test]
        fn arbitrary_sequences(seq in proptest::collection::vec(transition(), 0..64)) {
            let mut l = Lifecycle::default();
            let mut started = false;
            for t in seq {
                let before = l.state();
                match l.apply(t) {
                    Ok(after) => {
                        if t == Transition::Start { started = true; }
                        if t == Transition::Fail { prop_assert_eq!(after, State::Failed); }
                    }
                    Err(_) => prop_assert_eq!(l.state(), before),
                }
                if l.is_active() { prop_assert!(started); }
            }
        }
    }
}
