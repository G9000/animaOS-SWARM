use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;

use crate::connectors::gcalendar::{
    CalendarError, CalendarEventDraft, CalendarManager, CalendarWriteOperation,
};

use super::ToolExecutionContext;

fn calendar_manager(context: &ToolExecutionContext) -> Result<CalendarManager, String> {
    context
        .calendar
        .clone()
        .ok_or_else(|| "calendar integration is unavailable on this daemon".to_string())
}

fn required_string(tool_call: &ToolCall, name: &str, tool: &str) -> Result<String, String> {
    match tool_call.args.get(name) {
        Some(DataValue::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(format!("{tool} {name} must be a non-empty string")),
    }
}

fn optional_string(tool_call: &ToolCall, name: &str) -> Option<String> {
    match tool_call.args.get(name) {
        Some(DataValue::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn calendar_error(error: CalendarError) -> String {
    match error {
        CalendarError::NotConnected => {
            "Google Calendar is not connected for this agent. Ask the owner to connect it in Connectors → Google Calendar."
                .to_string()
        }
        CalendarError::ReauthRequired => {
            "Google Calendar access expired. Ask the owner to reconnect in Connectors → Google Calendar."
                .to_string()
        }
        CalendarError::Unconfigured => {
            "Google Calendar OAuth is not configured on this daemon (missing ANIMA_GOOGLE_CLIENT_ID/ANIMA_GOOGLE_CLIENT_SECRET)."
                .to_string()
        }
        CalendarError::InvalidDraft => "the calendar event details are invalid".to_string(),
        CalendarError::Conflict => {
            "too many pending calendar changes are awaiting approval; ask the owner to resolve them in Connectors → Google Calendar first"
                .to_string()
        }
        _ => format!("calendar request failed: {error:?}"),
    }
}

fn draft_from_args(
    tool_call: &ToolCall,
    require_event_id: bool,
) -> Result<CalendarEventDraft, String> {
    let event_id = optional_string(tool_call, "event_id");
    if require_event_id && event_id.is_none() {
        return Err("event_id must be a non-empty string".to_string());
    }
    Ok(CalendarEventDraft {
        calendar_id: optional_string(tool_call, "calendar_id").unwrap_or_default(),
        event_id,
        title: optional_string(tool_call, "title").unwrap_or_default(),
        start: optional_string(tool_call, "start").unwrap_or_default(),
        end: optional_string(tool_call, "end").unwrap_or_default(),
        location: optional_string(tool_call, "location"),
        description: optional_string(tool_call, "description"),
    })
}

pub(super) fn execute_calendar_list_events(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let result = async {
            let manager = calendar_manager(&context)?;
            let time_min = required_string(&tool_call, "time_min", "calendar_list_events")?;
            let time_max = required_string(&tool_call, "time_max", "calendar_list_events")?;
            let calendar_id = optional_string(&tool_call, "calendar_id");
            manager
                .list_events_for_agent(
                    agent.id.to_string().as_str(),
                    calendar_id.as_deref(),
                    &time_min,
                    &time_max,
                )
                .await
                .map_err(calendar_error)
        }
        .await;

        match result {
            Ok(events) => {
                let text = if events.is_empty() {
                    "No events in that window.".to_string()
                } else {
                    let mut lines = vec![format!("{} event(s):", events.len())];
                    for event in events {
                        let mut line = format!(
                            "- \"{}\" {} → {} (id: {})",
                            event.title, event.start, event.end, event.id
                        );
                        if let Some(location) = event.location {
                            line.push_str(&format!(" @ {location}"));
                        }
                        lines.push(line);
                    }
                    lines.join("\n")
                };
                TaskResult::success(
                    Content {
                        text,
                        ..Default::default()
                    },
                    0,
                )
            }
            Err(message) => TaskResult::error(message, 0),
        }
    })
}

fn submit_write(
    context: ToolExecutionContext,
    agent: AgentState,
    tool_call: ToolCall,
    operation: CalendarWriteOperation,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let result = async {
            let manager = calendar_manager(&context)?;
            let draft = draft_from_args(&tool_call, require_event_id(operation))?;
            manager
                .submit_write(agent.id.to_string().as_str(), operation, draft)
                .await
                .map_err(calendar_error)
        }
        .await;

        match result {
            Ok(write) => TaskResult::success(
                Content {
                    text: format!(
                        "{} — pending owner confirmation (id: {}). Tell the user to approve or reject it in Connectors → Google Calendar.",
                        write.summary, write.id
                    ),
                    ..Default::default()
                },
                0,
            ),
            Err(message) => TaskResult::error(message, 0),
        }
    })
}

fn require_event_id(operation: CalendarWriteOperation) -> bool {
    matches!(
        operation,
        CalendarWriteOperation::Update | CalendarWriteOperation::Delete
    )
}

pub(super) fn execute_calendar_create_event(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    submit_write(context, agent, tool_call, CalendarWriteOperation::Create)
}

pub(super) fn execute_calendar_update_event(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    submit_write(context, agent, tool_call, CalendarWriteOperation::Update)
}

pub(super) fn execute_calendar_delete_event(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    submit_write(context, agent, tool_call, CalendarWriteOperation::Delete)
}
