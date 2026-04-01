#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    HealthCheck,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineEvent {
    pub event_type: EventType,
    pub timestamp_ms: u128,
}
