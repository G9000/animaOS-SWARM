//! Context-window selection (spec §5.2): which of a session's earlier
//! messages a run sends to the model within its token budget. Pure: hosts
//! pass the history, an optional summary, the budget, and an estimator.

use std::borrow::Borrow;

use crate::primitives::{AttachmentType, Message, MessageRole};
use crate::runtime_serde::data_value_json;

/// Characters per estimated token (spec §5.2).
pub const CHARS_PER_TOKEN: u64 = 4;
/// Tokens added per message for role and framing.
pub const MESSAGE_OVERHEAD_TOKENS: u64 = 8;
/// The calibration factor is clamped to this range (spec §5.2).
pub const MIN_CALIBRATION: f64 = 0.5;
pub const MAX_CALIBRATION: f64 = 2.0;
/// Images sent as images per window; older ones become text (spec §5.2).
pub const MAX_CONTEXT_IMAGES: usize = 4;

/// Where each turn of `messages` starts, oldest first; `messages` are one
/// room's messages in transcript order. A turn starts at a user message and
/// holds every following assistant, tool, and system message up to the next
/// user message, so a cut made only at these indices never separates an
/// assistant tool-call message from its tool results (providers reject a
/// tool result whose call is missing). Messages before the first user
/// message end a turn whose start is gone. Context selection, hot-tail
/// pruning, and the silent check-in grouping all cut here.
pub fn turn_starts<M: Borrow<Message>>(
    messages: &[M],
) -> impl DoubleEndedIterator<Item = usize> + '_ {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| Borrow::<Message>::borrow(*message).role == MessageRole::User)
        .map(|(index, _)| index)
}

/// Estimates tokens as characters ÷ 4 plus 8 per message plus serialized
/// tool-call arguments ÷ 4, times a per-session calibration (spec §5.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenEstimator {
    calibration: f64,
}

impl Default for TokenEstimator {
    fn default() -> Self {
        Self { calibration: 1.0 }
    }
}

impl TokenEstimator {
    pub fn new(calibration: f64) -> Self {
        Self {
            calibration: clamp_calibration(calibration),
        }
    }

    pub fn calibration(&self) -> f64 {
        self.calibration
    }

    /// The uncalibrated estimate of one message.
    pub fn raw_message_tokens(message: &Message) -> u64 {
        let text = message.content.text.chars().count() as u64;
        let calls = message
            .content
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("toolCalls"))
            .map_or(0, |calls| data_value_json(calls).chars().count() as u64);
        text.div_ceil(CHARS_PER_TOKEN) + MESSAGE_OVERHEAD_TOKENS + calls.div_ceil(CHARS_PER_TOKEN)
    }

    pub fn message_tokens(&self, message: &Message) -> u64 {
        self.scale(Self::raw_message_tokens(message))
    }

    /// The estimate of `text` sent as one message (a summary or an input).
    pub fn text_tokens(&self, text: &str) -> u64 {
        self.scale(
            (text.chars().count() as u64).div_ceil(CHARS_PER_TOKEN) + MESSAGE_OVERHEAD_TOKENS,
        )
    }

    fn scale(&self, tokens: u64) -> u64 {
        (tokens as f64 * self.calibration).ceil() as u64
    }
}

/// The next run's calibration from this run's first model call: the
/// provider-reported prompt tokens over the estimate, clamped (spec §5.2).
pub fn calibration_factor(reported_prompt_tokens: u64, estimated_tokens: u64) -> f64 {
    if reported_prompt_tokens == 0 || estimated_tokens == 0 {
        return 1.0;
    }
    clamp_calibration(reported_prompt_tokens as f64 / estimated_tokens as f64)
}

fn clamp_calibration(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(MIN_CALIBRATION, MAX_CALIBRATION)
    } else {
        1.0
    }
}

/// A session summary (spec §5.4): text standing in for every message up to
/// and including `through_message_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextSummary {
    pub text: String,
    pub through_message_id: String,
}

/// What a run sends and what it leaves out (spec §5.2–§5.3).
#[derive(Clone, Debug, PartialEq)]
pub struct ContextSelection {
    /// Whole turns, oldest first, with the image cap applied.
    pub messages: Vec<Message>,
    /// Whole turns left out that no summary covers, oldest first.
    pub dropped: Vec<Message>,
    /// Messages the summary covers.
    pub summarized: usize,
    /// The estimate of `messages` plus the summary.
    pub estimated_tokens: u64,
}

/// Selects `history` (model-visible, oldest first) within `budget_tokens`.
pub fn select_context(
    history: &[Message],
    summary: Option<&ContextSummary>,
    budget_tokens: u64,
    estimator: &TokenEstimator,
) -> ContextSelection {
    let covered = summary
        .and_then(|summary| {
            history
                .iter()
                .position(|message| message.id == summary.through_message_id)
        })
        .map_or(0, |index| index + 1);
    let candidates = &history[covered..];
    let summary_tokens = summary.map_or(0, |summary| estimator.text_tokens(&summary.text));
    let starts: Vec<usize> = turn_starts(candidates).collect();
    let Some(&first_turn) = starts.first() else {
        return ContextSelection {
            messages: Vec::new(),
            dropped: Vec::new(),
            summarized: covered,
            estimated_tokens: summary_tokens,
        };
    };
    let mut remaining = budget_tokens.saturating_sub(summary_tokens);
    let mut spent = 0u64;
    let mut keep_from = candidates.len();
    for (index, &start) in starts.iter().enumerate().rev() {
        let end = starts.get(index + 1).copied().unwrap_or(candidates.len());
        let cost: u64 = candidates[start..end]
            .iter()
            .map(|message| estimator.message_tokens(message))
            .sum();
        if cost > remaining {
            break;
        }
        remaining -= cost;
        spent += cost;
        keep_from = start;
    }
    let mut messages = candidates[keep_from..].to_vec();
    cap_images(&mut messages);
    ContextSelection {
        messages,
        dropped: candidates[first_turn..keep_from].to_vec(),
        summarized: covered,
        estimated_tokens: summary_tokens + spent,
    }
}

/// Keeps the newest `MAX_CONTEXT_IMAGES` image attachments; older ones
/// become `[image: <name> (<data>)]` lines of their message's text.
fn cap_images(messages: &mut [Message]) {
    let mut kept = 0;
    for message in messages.iter_mut().rev() {
        let Some(attachments) = message.content.attachments.take() else {
            continue;
        };
        let mut remaining = Vec::with_capacity(attachments.len());
        let mut notes = Vec::new();
        for attachment in attachments {
            if attachment.attachment_type != AttachmentType::Image {
                remaining.push(attachment);
            } else if kept < MAX_CONTEXT_IMAGES {
                kept += 1;
                remaining.push(attachment);
            } else {
                notes.push(format!(
                    "[image: {} ({})]",
                    attachment.name, attachment.data
                ));
            }
        }
        message.content.attachments = (!remaining.is_empty()).then_some(remaining);
        for note in notes {
            message.content.text.push('\n');
            message.content.text.push_str(&note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{Attachment, AttachmentType, Content, DataValue, Message, MessageRole};
    use std::collections::BTreeMap;

    /// A message whose text is `chars` characters: `chars / 4` tokens plus
    /// the 8-token overhead.
    fn message(id: &str, role: MessageRole, chars: usize) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent-1".into(),
            room_id: "chat:a".into(),
            content: Content {
                text: "x".repeat(chars),
                ..Content::default()
            },
            role,
            created_at_ms: 1,
        }
    }

    /// A user + assistant turn of 16 + 16 tokens.
    fn turn(n: usize) -> Vec<Message> {
        vec![
            message(&format!("u{n}"), MessageRole::User, 32),
            message(&format!("a{n}"), MessageRole::Assistant, 32),
        ]
    }

    fn ids(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|message| message.id.as_str()).collect()
    }

    fn image(name: &str) -> Attachment {
        Attachment {
            attachment_type: AttachmentType::Image,
            name: name.into(),
            data: format!("uploads/{name}"),
        }
    }

    #[test]
    fn turn_starts_mark_each_user_message() {
        let messages = vec![
            message("t", MessageRole::Tool, 4),
            message("u1", MessageRole::User, 4),
            message("a1", MessageRole::Assistant, 4),
            message("s1", MessageRole::System, 4),
            message("u2", MessageRole::User, 4),
        ];
        assert_eq!(turn_starts(&messages).collect::<Vec<_>>(), [1, 4]);
        assert_eq!(turn_starts(&messages).rev().next(), Some(4));
        let borrowed: Vec<&Message> = messages.iter().collect();
        assert_eq!(turn_starts(&borrowed).collect::<Vec<_>>(), [1, 4]);
    }

    #[test]
    fn estimates_count_characters_overhead_tool_arguments_and_calibration() {
        let plain = message("m", MessageRole::User, 40);
        assert_eq!(TokenEstimator::raw_message_tokens(&plain), 10 + 8);
        assert_eq!(TokenEstimator::default().message_tokens(&plain), 18);
        assert_eq!(TokenEstimator::new(1.5).message_tokens(&plain), 27);
        let mut calls = message("c", MessageRole::Assistant, 0);
        calls.content.metadata = Some(BTreeMap::from([(
            "toolCalls".to_string(),
            DataValue::Array(vec![DataValue::String("ab".repeat(10))]),
        )]));
        // `["abab…"]` is 24 characters: 6 tokens on top of the overhead.
        assert_eq!(TokenEstimator::raw_message_tokens(&calls), 8 + 6);
        assert_eq!(TokenEstimator::default().text_tokens("abcd"), 1 + 8);
        assert_eq!(TokenEstimator::new(0.1).calibration(), MIN_CALIBRATION);
        assert_eq!(TokenEstimator::new(9.0).calibration(), MAX_CALIBRATION);
        assert_eq!(TokenEstimator::new(f64::NAN).calibration(), 1.0);
    }

    #[test]
    fn calibration_is_reported_over_estimated_and_clamped() {
        assert_eq!(calibration_factor(150, 100), 1.5);
        assert_eq!(calibration_factor(1_000, 100), MAX_CALIBRATION);
        assert_eq!(calibration_factor(10, 100), MIN_CALIBRATION);
        assert_eq!(calibration_factor(0, 100), 1.0);
        assert_eq!(calibration_factor(100, 0), 1.0);
    }

    #[test]
    fn selection_keeps_the_newest_whole_turns_within_the_budget() {
        let history: Vec<Message> = (1..=3).flat_map(turn).collect();

        let selection = select_context(&history, None, 64, &TokenEstimator::default());

        assert_eq!(ids(&selection.messages), ["u2", "a2", "u3", "a3"]);
        assert_eq!(ids(&selection.dropped), ["u1", "a1"]);
        assert_eq!(selection.summarized, 0);
        assert_eq!(selection.estimated_tokens, 64);
        let everything = select_context(&history, None, 1_000, &TokenEstimator::default());
        assert_eq!(everything.messages.len(), 6);
        assert!(everything.dropped.is_empty());
    }

    #[test]
    fn a_tool_call_turn_is_never_split_from_its_results() {
        let mut history = turn(1);
        history.extend([
            message("u2", MessageRole::User, 32),
            message("call", MessageRole::Assistant, 32),
            message("result", MessageRole::Tool, 32),
            message("a2", MessageRole::Assistant, 32),
        ]);
        history.extend(turn(3));

        // 80 tokens fit the newest turn (32) but not the 64-token tool turn.
        let selection = select_context(&history, None, 80, &TokenEstimator::default());

        assert_eq!(ids(&selection.messages), ["u3", "a3"]);
        assert_eq!(
            ids(&selection.dropped),
            ["u1", "a1", "u2", "call", "result", "a2"]
        );
    }

    #[test]
    fn a_history_that_starts_mid_turn_loses_its_leading_messages() {
        let mut history = vec![
            message("orphan-result", MessageRole::Tool, 32),
            message("orphan-reply", MessageRole::Assistant, 32),
        ];
        history.extend(turn(1));

        let selection = select_context(&history, None, 1_000, &TokenEstimator::default());

        assert_eq!(ids(&selection.messages), ["u1", "a1"]);
        assert!(
            selection.dropped.is_empty(),
            "a turn whose opening is gone is structural, not trimmed context"
        );
    }

    #[test]
    fn a_single_turn_larger_than_the_budget_is_dropped_whole() {
        let mut history = turn(1);
        history.extend([
            message("u2", MessageRole::User, 32),
            message("huge", MessageRole::Tool, 4_000),
        ]);

        let selection = select_context(&history, None, 100, &TokenEstimator::default());

        assert!(
            selection.messages.is_empty(),
            "the current message still goes alone"
        );
        assert_eq!(ids(&selection.dropped), ["u1", "a1", "u2", "huge"]);
        assert_eq!(selection.estimated_tokens, 0);
    }

    #[test]
    fn a_summary_covers_its_messages_and_counts_against_the_budget() {
        let history: Vec<Message> = (1..=3).flat_map(turn).collect();
        let summary = ContextSummary {
            text: "s".repeat(64),
            through_message_id: "a1".into(),
        };

        // 24 summary tokens + 32 fit exactly one uncovered turn.
        let selection = select_context(&history, Some(&summary), 56, &TokenEstimator::default());

        assert_eq!(ids(&selection.messages), ["u3", "a3"]);
        assert_eq!(ids(&selection.dropped), ["u2", "a2"]);
        assert_eq!(selection.summarized, 2);
        assert_eq!(selection.estimated_tokens, 56);

        let gone = ContextSummary {
            text: "s".repeat(64),
            through_message_id: "pruned-long-ago".into(),
        };
        let selection = select_context(&history, Some(&gone), 120, &TokenEstimator::default());
        assert_eq!(selection.messages.len(), 6);
        assert_eq!(selection.summarized, 0);
        assert_eq!(selection.estimated_tokens, 120, "the summary still counts");
    }

    #[test]
    fn only_the_newest_four_images_are_sent_as_images() {
        let mut history = Vec::new();
        for n in 1..=3 {
            let mut user = message(&format!("u{n}"), MessageRole::User, 4);
            user.content.attachments = Some(vec![
                image(&format!("{n}-a.png")),
                image(&format!("{n}-b.png")),
            ]);
            history.push(user);
            history.push(message(&format!("a{n}"), MessageRole::Assistant, 4));
        }

        let selection = select_context(&history, None, 10_000, &TokenEstimator::default());

        let oldest = &selection.messages[0];
        assert_eq!(oldest.content.attachments, None);
        assert!(oldest
            .content
            .text
            .ends_with("\n[image: 1-a.png (uploads/1-a.png)]\n[image: 1-b.png (uploads/1-b.png)]"));
        let kept: usize = selection
            .messages
            .iter()
            .filter_map(|message| message.content.attachments.as_ref())
            .map(Vec::len)
            .sum();
        assert_eq!(kept, MAX_CONTEXT_IMAGES);
        assert!(
            history[0].content.attachments.is_some(),
            "the history is untouched"
        );
    }
}
