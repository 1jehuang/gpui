//! Shared GPUI input semantics projected onto Android's EditorInfo contract.
//! Constants are the Android InputType/EditorInfo values, kept platform-free so
//! the mapping is tested on hosts without an Android VM.
use gpui::{Autocapitalize, TextInputAction, TextInputConfiguration, TextInputPurpose};
use std::cell::Cell;

/// Keyboard attributes passed to an embedding activity's non-drawing IME host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AndroidTextInputConfiguration {
    pub input_type: i32,
    pub ime_options: i32,
    /// Android autofill hint, or the empty string when no content hint is known.
    pub autofill_hint: &'static str,
}

#[derive(Default)]
pub(crate) struct ConfigurationDelivery {
    requested: Cell<Option<AndroidTextInputConfiguration>>,
    delivered: Cell<Option<AndroidTextInputConfiguration>>,
}

impl ConfigurationDelivery {
    pub(crate) fn request(&self, configuration: AndroidTextInputConfiguration) {
        self.requested.set(Some(configuration));
    }

    pub(crate) fn apply(&self, deliver: impl FnOnce(AndroidTextInputConfiguration) -> bool) {
        let Some(configuration) = self.requested.get() else {
            return;
        };
        if self.delivered.get() != Some(configuration) && deliver(configuration) {
            self.delivered.set(Some(configuration));
        }
    }
}

impl From<&TextInputConfiguration> for AndroidTextInputConfiguration {
    fn from(configuration: &TextInputConfiguration) -> Self {
        let mut input_type = match configuration.purpose {
            TextInputPurpose::Text => 0x0000_0001 | 0x0002_0000,
            TextInputPurpose::Email => 0x0000_0001 | 0x0000_0020,
            TextInputPurpose::OneTimeCode | TextInputPurpose::Number => 0x0000_0002,
        };
        if input_type & 0xf == 1 {
            input_type |= match configuration.autocapitalize {
                Autocapitalize::None => 0,
                Autocapitalize::Characters => 0x0000_1000,
                Autocapitalize::Words => 0x0000_2000,
                Autocapitalize::Sentences => 0x0000_4000,
            };
            if configuration.suggestions {
                if configuration.autocorrect {
                    input_type |= 0x0000_8000;
                }
            } else {
                // Android gives NO_SUGGESTIONS precedence over AUTO_CORRECT.
                input_type |= 0x0008_0000;
            }
        }
        let action = match configuration.input_action {
            TextInputAction::Unspecified => 0,
            TextInputAction::Enter => 1,
            TextInputAction::Go => 2,
            TextInputAction::Search => 3,
            TextInputAction::Send => 4,
            TextInputAction::Next => 5,
            TextInputAction::Done => 6,
            TextInputAction::Previous => 7,
        };
        let autofill_hint = match configuration.purpose {
            TextInputPurpose::Email => "emailAddress",
            TextInputPurpose::OneTimeCode => "smsOTPCode",
            TextInputPurpose::Text | TextInputPurpose::Number => "",
        };
        Self {
            input_type,
            // Keep GPUI as the only visual editor, not the IME's fullscreen UI.
            ime_options: action
                | 0x1000_0000
                | 0x0200_0000
                | if configuration.purpose == TextInputPurpose::OneTimeCode {
                    0x0100_0000 // IME_FLAG_NO_PERSONALIZED_LEARNING (API 26).
                } else {
                    0
                },
            autofill_hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_configuration_never_reaches_activity_again() {
        let delivery = ConfigurationDelivery::default();
        let calls = Cell::new(0);
        let configuration = AndroidTextInputConfiguration::from(&TextInputConfiguration::default());
        delivery.apply(|_| panic!("nothing requested"));
        for _ in 0..100 {
            delivery.request(configuration);
            delivery.apply(|received| {
                assert_eq!(received, configuration);
                calls.set(calls.get() + 1);
                true
            });
        }
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn failed_delivery_retries_and_a_new_request_replaces_stale_pending_hints() {
        let delivery = ConfigurationDelivery::default();
        let initial = AndroidTextInputConfiguration::from(&TextInputConfiguration::default());
        let email = AndroidTextInputConfiguration::from(&TextInputConfiguration {
            purpose: TextInputPurpose::Email,
            ..Default::default()
        });
        delivery.request(initial);
        delivery.apply(|_| false);
        delivery.request(email);
        delivery.apply(|received| {
            assert_eq!(received, email);
            false
        });
        delivery.apply(|received| {
            assert_eq!(received, email);
            true
        });
        delivery.apply(|_| panic!("successful delivery must not repeat"));
    }

    #[test]
    fn email_is_single_line_without_unrequested_caps_or_correction() {
        let configuration = TextInputConfiguration {
            purpose: TextInputPurpose::Email,
            input_action: TextInputAction::Next,
            ..Default::default()
        };
        let android = AndroidTextInputConfiguration::from(&configuration);
        assert_eq!(android.input_type, 0x0008_0021);
        assert_eq!(android.ime_options, 0x1200_0005);
        assert_eq!(android.autofill_hint, "emailAddress");
    }

    #[test]
    fn numeric_and_otp_keyboards_do_not_inherit_text_flags() {
        for purpose in [TextInputPurpose::Number, TextInputPurpose::OneTimeCode] {
            let android = AndroidTextInputConfiguration::from(&TextInputConfiguration {
                purpose,
                autocapitalize: Autocapitalize::Sentences,
                autocorrect: true,
                suggestions: true,
                input_action: TextInputAction::Go,
            });
            assert_eq!(android.input_type, 2);
            if purpose == TextInputPurpose::OneTimeCode {
                assert_eq!(android.autofill_hint, "smsOTPCode");
                assert_eq!(android.ime_options, 0x1300_0002);
            } else {
                assert_eq!(android.autofill_hint, "");
                assert_eq!(android.ime_options, 0x1200_0002);
            }
        }
    }

    #[test]
    fn composer_retains_multiline_and_requested_text_assistance() {
        let android = AndroidTextInputConfiguration::from(&TextInputConfiguration {
            purpose: TextInputPurpose::Text,
            autocapitalize: Autocapitalize::Sentences,
            autocorrect: true,
            suggestions: true,
            input_action: TextInputAction::Send,
        });
        assert_eq!(android.input_type, 0x0002_c001);
        assert_eq!(android.ime_options, 0x1200_0004);
    }

    #[test]
    fn capitalization_and_all_action_hints_are_mapped() {
        for (autocapitalize, flag) in [
            (Autocapitalize::None, 0),
            (Autocapitalize::Characters, 0x1000),
            (Autocapitalize::Words, 0x2000),
            (Autocapitalize::Sentences, 0x4000),
        ] {
            let android = AndroidTextInputConfiguration::from(&TextInputConfiguration {
                autocapitalize,
                ..Default::default()
            });
            assert_eq!(android.input_type & 0x7000, flag);
        }
        for (input_action, value) in [
            (TextInputAction::Unspecified, 0),
            (TextInputAction::Enter, 1),
            (TextInputAction::Go, 2),
            (TextInputAction::Search, 3),
            (TextInputAction::Send, 4),
            (TextInputAction::Next, 5),
            (TextInputAction::Done, 6),
            (TextInputAction::Previous, 7),
        ] {
            let android = AndroidTextInputConfiguration::from(&TextInputConfiguration {
                input_action,
                ..Default::default()
            });
            assert_eq!(android.ime_options & 0xff, value);
        }
    }

    #[test]
    fn suggestions_disabled_takes_precedence_over_autocorrect() {
        let android = AndroidTextInputConfiguration::from(&TextInputConfiguration {
            autocorrect: true,
            suggestions: false,
            ..Default::default()
        });
        assert_eq!(android.input_type & 0x8000, 0);
        assert_ne!(android.input_type & 0x80000, 0);
        let again = AndroidTextInputConfiguration::from(&TextInputConfiguration {
            autocorrect: false,
            suggestions: false,
            ..Default::default()
        });
        assert_eq!(android, again);
    }
}
