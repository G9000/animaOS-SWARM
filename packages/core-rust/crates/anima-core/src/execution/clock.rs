use std::time::Duration;

pub(super) async fn wait_for_wall_clock_ms(delay_ms: u64) {
    if delay_ms > 0 {
        futures_timer::Delay::new(Duration::from_millis(delay_ms)).await;
    }
}
