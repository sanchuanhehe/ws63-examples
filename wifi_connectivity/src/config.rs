use embassy_time::Duration;
use hisi_rf::{BackendTimeout, OperationTimeout, RadioConfig, WifiConfig, WorkBudget};

pub const SCAN_RESULT_DEPTH: usize = 32;
pub const RUNNER_BUDGET: WorkBudget =
    WorkBudget::try_new(8, 100_000).expect("non-zero incremental work budget");

pub const SCAN_OPERATION_TIMEOUT: OperationTimeout =
    OperationTimeout::try_from_millis(15_000).expect("non-zero scan operation timeout");
pub const CONNECT_OPERATION_TIMEOUT: OperationTimeout =
    OperationTimeout::try_from_millis(60_000).expect("non-zero connect operation timeout");

pub const INITIALIZE_WAIT_DEADLINE: Duration = Duration::from_secs(35);
pub const SCAN_WAIT_DEADLINE: Duration = Duration::from_secs(30);
pub const CONNECT_WAIT_DEADLINE: Duration = Duration::from_secs(90);
pub const EVENT_WAIT_DEADLINE: Duration = Duration::from_secs(2);

/// Optional pre-radio delay used to sequence two-board HIL when the rig cannot
/// independently reset both targets. The default application path never waits.
pub const HIL_START_DELAY_MS: u32 = match option_env!("WS63_HIL_START_DELAY_MS") {
    Some(value) => parse_decimal_u32(value),
    None => 0,
};

/// Public DNS targets used to prove routed UDP connectivity.
pub const PUBLIC_DNS_TARGETS: [[u8; 4]; 2] = [[223, 5, 5, 5], [180, 76, 76, 76]];

const fn parse_decimal_u32(value: &str) -> u32 {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        panic!("WS63_HIL_START_DELAY_MS must be a decimal u32");
    }

    let mut index = 0;
    let mut parsed = 0_u32;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte < b'0' || byte > b'9' {
            panic!("WS63_HIL_START_DELAY_MS must be a decimal u32");
        }
        let digit = (byte - b'0') as u32;
        if parsed > (u32::MAX - digit) / 10 {
            panic!("WS63_HIL_START_DELAY_MS exceeds u32");
        }
        parsed = parsed * 10 + digit;
        index += 1;
    }
    parsed
}

#[cfg(feature = "dual-board-hil")]
#[path = "../../hil_wifi_config.rs"]
mod dual_board;

#[cfg(feature = "dual-board-hil")]
pub const TEST_SSID: &[u8] = dual_board::SSID;
#[cfg(feature = "dual-board-hil")]
pub const TEST_PASSPHRASE: &[u8] = dual_board::PASSPHRASE;

#[cfg(not(feature = "dual-board-hil"))]
pub const TEST_SSID: &[u8] = match option_env!("WS63_WIFI_SSID") {
    Some(value) => value.as_bytes(),
    None => b"",
};
#[cfg(not(feature = "dual-board-hil"))]
pub const TEST_PASSPHRASE: &[u8] = match option_env!("WS63_WIFI_PASSPHRASE") {
    Some(value) => value.as_bytes(),
    None => b"",
};

pub fn radio_config() -> RadioConfig {
    let mut config = RadioConfig::default();
    config.wifi = WifiConfig {
        initialize_timeout: BackendTimeout::try_from_millis(30_000)
            .expect("non-zero backend initialize timeout"),
        disconnect_timeout: BackendTimeout::try_from_millis(10_000)
            .expect("non-zero backend disconnect timeout"),
    };
    config
}
