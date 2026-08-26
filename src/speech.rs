//! Direct speech and braille output via Prism.
//!
//! This complements, rather than duplicates, what the native controls say.
//! wxWidgets exposes *what is focused* to the platform's accessibility API
//! and the user's screen reader announces it. Prism handles output that has
//! no focused control behind it: load progress, errors, feed changes, and
//! explicit "read this to me now" requests. Keeping the two responsibilities
//! disjoint is what stops every action being spoken twice.

use prism::{Backend, BackendFeatures, BackendId, Context};

/// Backends that are speech engines rather than screen readers.
///
/// The distinction drives one real decision: if a screen reader is running it
/// announces focus changes from the native controls, so we must stay quiet.
/// If Prism only found a bare TTS engine, nothing else is speaking and we
/// have to announce focus ourselves.
const TTS_ONLY: [BackendId; 6] = [
    BackendId::SAPI,
    BackendId::ONE_CORE,
    BackendId::AV_SPEECH,
    BackendId::ANDROID_TTS,
    BackendId::WEB_SPEECH,
    BackendId::SPIEL,
];

pub struct Speaker {
    // Declaration order is load-bearing: `Backend` must drop before `Context`,
    // because dropping the context shuts the library down underneath it.
    backend: Option<Backend>,
    _context: Option<Context>,
    features: BackendFeatures,
    enabled: bool,
    /// True when a screen reader is driving output, meaning it already
    /// announces focus from the controls themselves and we should not.
    screen_reader: bool,
    status: String,
}

impl Speaker {
    /// Connect to the best available screen reader or TTS backend.
    ///
    /// Failure is never fatal: with no backend the app is still fully usable
    /// through the screen reader's own reading of the native controls, so we
    /// record why and carry on silently.
    pub fn new() -> Self {
        match Self::connect() {
            Ok(speaker) => speaker,
            Err(err) => Speaker {
                backend: None,
                _context: None,
                features: BackendFeatures::empty(),
                enabled: false,
                screen_reader: false,
                status: format!("speech unavailable: {err}"),
            },
        }
    }

    fn connect() -> prism::Result<Self> {
        let context = Context::new()?;
        let backend = context.acquire_best()?;
        let features = backend.features();
        let name = backend.name().unwrap_or_else(|_| "unknown".to_string());
        let id = identify(&context, &name);
        let screen_reader = id.map(|id| !TTS_ONLY.contains(&id)).unwrap_or(false);

        Ok(Speaker {
            backend: Some(backend),
            _context: Some(context),
            features,
            enabled: true,
            screen_reader,
            status: if screen_reader {
                format!("screen reader: {name}")
            } else {
                format!("speech engine: {name}")
            },
        })
    }

    /// A human-readable description of the speech backend, for the help screen.
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled && self.backend.is_some()
    }

    /// Whether *we* are responsible for announcing focus changes.
    ///
    /// Only true when Prism is driving a bare TTS engine: with a screen reader
    /// attached, the list or tree already announces the focused row and
    /// speaking here would double every movement.
    pub fn announces_focus(&self) -> bool {
        self.is_enabled() && !self.screen_reader
    }

    /// Turn Prism output on or off, returning the new state. Users running a
    /// screen reader that already announces everything may prefer it off.
    pub fn toggle(&mut self) -> bool {
        if self.backend.is_none() {
            return false;
        }
        self.enabled = !self.enabled;
        if !self.enabled {
            self.stop();
        }
        self.enabled
    }

    /// Speak `text`, interrupting anything in progress.
    ///
    /// Prefers `output`, which routes to a braille display as well as speech
    /// when the backend supports both.
    pub fn announce(&mut self, text: &str) {
        if !self.enabled || text.is_empty() {
            return;
        }
        let Some(backend) = self.backend.as_mut() else {
            return;
        };

        // Errors here are deliberately swallowed: a backend that has gone away
        // mid-session must not take the UI down with it.
        if self.features.contains(BackendFeatures::SUPPORTS_OUTPUT) {
            let _ = backend.output(text, true);
        } else if self.features.contains(BackendFeatures::SUPPORTS_SPEAK) {
            let _ = backend.speak(text, true);
        }
    }

    /// Stop speech in progress.
    pub fn stop(&mut self) {
        if !self.features.contains(BackendFeatures::SUPPORTS_STOP) {
            return;
        }
        if let Some(backend) = self.backend.as_mut() {
            let _ = backend.stop();
        }
    }
}

impl Default for Speaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the registry id whose registered name matches the acquired backend.
///
/// Prism hands back a `Backend` without its id, so we match on name against the
/// registry to learn which backend we actually got.
fn identify(context: &Context, name: &str) -> Option<BackendId> {
    if let Some(id) = context.id_by_name(name) {
        return Some(id);
    }
    (0..context.backend_count())
        .filter_map(|i| context.id_at(i))
        .find(|id| {
            context
                .name_of(*id)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                || id
                    .well_known_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
}
