#[allow(dead_code)]
pub enum VoiceState {
    Idle,
    WakeWordDetected,
    Recording { buf: Vec<i16> },
    Transcribing { audio: Vec<i16> },
    AgentThinking { transcription: String },
    Synthesizing { response: String },
    Playing,
}

#[allow(dead_code)]
pub enum VoiceEvent {
    WakeWordDetected,
    SilenceDetected,
    TranscriptionReady(String),
    ResponseReady(String),
    PlaybackDone,
    Error(String),
}

#[allow(dead_code)]
impl VoiceState {
    pub fn transition(self, event: VoiceEvent) -> Result<VoiceState, String> {
        match (self, event) {
            (VoiceState::Idle, VoiceEvent::WakeWordDetected) => Ok(VoiceState::WakeWordDetected),
            (VoiceState::WakeWordDetected, VoiceEvent::WakeWordDetected) => {
                Ok(VoiceState::Recording { buf: Vec::new() })
            }
            (VoiceState::WakeWordDetected, _) => Ok(VoiceState::Recording { buf: Vec::new() }),
            (VoiceState::Recording { buf }, VoiceEvent::SilenceDetected) => {
                Ok(VoiceState::Transcribing { audio: buf })
            }
            (VoiceState::Transcribing { .. }, VoiceEvent::TranscriptionReady(t)) => {
                Ok(VoiceState::AgentThinking { transcription: t })
            }
            (VoiceState::AgentThinking { .. }, VoiceEvent::ResponseReady(r)) => {
                Ok(VoiceState::Synthesizing { response: r })
            }
            (VoiceState::Synthesizing { .. }, VoiceEvent::PlaybackDone) => Ok(VoiceState::Playing),
            (VoiceState::Playing, VoiceEvent::PlaybackDone) => Ok(VoiceState::Idle),
            (_, VoiceEvent::Error(_)) => Ok(VoiceState::Idle),
            (state, event) => Err(format!(
                "invalid transition: {} + {}",
                state.name(),
                event.name()
            )),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            VoiceState::Idle => "Idle",
            VoiceState::WakeWordDetected => "WakeWordDetected",
            VoiceState::Recording { .. } => "Recording",
            VoiceState::Transcribing { .. } => "Transcribing",
            VoiceState::AgentThinking { .. } => "AgentThinking",
            VoiceState::Synthesizing { .. } => "Synthesizing",
            VoiceState::Playing => "Playing",
        }
    }
}

#[allow(dead_code)]
impl VoiceEvent {
    fn name(&self) -> &'static str {
        match self {
            VoiceEvent::WakeWordDetected => "WakeWordDetected",
            VoiceEvent::SilenceDetected => "SilenceDetected",
            VoiceEvent::TranscriptionReady(_) => "TranscriptionReady",
            VoiceEvent::ResponseReady(_) => "ResponseReady",
            VoiceEvent::PlaybackDone => "PlaybackDone",
            VoiceEvent::Error(_) => "Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_accepts_wake_event() {
        let s = VoiceState::Idle
            .transition(VoiceEvent::WakeWordDetected)
            .expect("valid transition");
        assert!(matches!(s, VoiceState::WakeWordDetected));
    }

    #[test]
    fn recording_to_transcribing_on_silence() {
        let buf = vec![1i16, 2, 3];
        let s = VoiceState::Recording { buf }
            .transition(VoiceEvent::SilenceDetected)
            .expect("valid transition");
        assert!(matches!(s, VoiceState::Transcribing { .. }));
    }

    #[test]
    fn invalid_transition_returns_err() {
        let result = VoiceState::Idle.transition(VoiceEvent::ResponseReady("hi".into()));
        assert!(result.is_err());
    }

    #[test]
    fn error_event_always_resets_to_idle() {
        let s = VoiceState::AgentThinking {
            transcription: "test".into(),
        }
        .transition(VoiceEvent::Error("oops".into()))
        .expect("error event should always be valid");
        assert!(matches!(s, VoiceState::Idle));
    }
}
