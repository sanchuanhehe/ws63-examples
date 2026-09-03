//! WS63 end-to-end connectivity through the public `hisi-rf` facade.
//!
//! The application uses one bounded radio runner for initialize, scan,
//! association, DHCP, repeated ICMP, and lease renewal. RF integration crates
//! remain transitive implementation details.

#![no_main]
#![no_std]

mod config;
mod dns_contract;
mod network_runner;

use core::num::{NonZeroU32, NonZeroUsize};

use embassy_executor::{Executor, Spawner};
use embassy_time::{Duration, Timer, with_timeout};
use hisi_hal::Peripherals;
use hisi_hal::delay::Delay;
use hisi_hal::rf_power::RfPower;
use hisi_hal::time::Instant;
use hisi_hal::uart::{Config as UartConfig, Uart, UartClock};
use hisi_hal::wdt::Watchdog;
use hisi_panic_handler as _;
#[cfg(feature = "wpa3")]
use hisi_rf::SaePwe;
use hisi_rf::ws63::{
    RadioParts, RadioRunner, RunnerDiagnosticsSnapshot, WaitDiagnosticsSnapshot, WifiDevice,
    declare_radio_storage,
};
use hisi_rf::{
    DiagnosticCode, Error as WifiError, IncrementalDriverEvent, Passphrase, ScanConfig,
    ScanOutcome, ScanResult, Security, StationConfig, WifiController, WifiEvent,
};
use hisi_riscv_rt::entry;
use static_cell::StaticCell;

#[cfg(all(feature = "wpa2", feature = "wpa3"))]
compile_error!("select exactly one station security profile: wpa2 or wpa3");
#[cfg(not(any(feature = "wpa2", feature = "wpa3")))]
compile_error!("select exactly one station security profile: wpa2 or wpa3");

use config::{
    CONNECT_OPERATION_TIMEOUT, CONNECT_WAIT_DEADLINE, EVENT_WAIT_DEADLINE,
    HIL_START_DELAY_MS, INITIALIZE_WAIT_DEADLINE, RUNNER_BUDGET, SCAN_OPERATION_TIMEOUT,
    SCAN_RESULT_DEPTH, SCAN_WAIT_DEADLINE, TEST_PASSPHRASE, TEST_SSID,
};

type Uart0 = Uart<'static, hisi_hal::peripherals::Uart0<'static>>;

declare_radio_storage!(static RADIO_STORAGE);
static RTOS_STORAGE: hisi_rtos::SchedulerStorage<15> = hisi_rtos::SchedulerStorage::new();
#[cfg_attr(target_arch = "riscv32", unsafe(link_section = ".hisi.shared-arena"))]
static RTOS_ARENA: hisi_rtos::SchedulerArena<{ hisi_rf::ws63::SELECTED_RUNTIME_ARENA_BYTES }> =
    hisi_rtos::SchedulerArena::new();
static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static UART: StaticCell<Uart0> = StaticCell::new();
static RADIO_PARTS: StaticCell<RadioParts> = StaticCell::new();

hisi_rtos::bind_interrupts!(struct RtosIrqs {
    TIMER_INT0 => hisi_rtos::ws63::TimerInterrupt;
    SOFT_INT0 => hisi_rtos::ws63::SoftwareInterrupt;
});

#[entry]
fn main() -> ! {
    let p = Peripherals::take().expect("peripherals already taken");
    let uart = UART.init(Uart::new_uart0(
        p.UART0,
        UartConfig {
            clock: UartClock::Boot,
            ..UartConfig::default()
        },
    ));
    Watchdog::new(p.WDT).disable();
    uart.write(b"\r\nRFDBG_CONNECTIVITY_BEGIN facade=hisi-rf\r\n");

    let mut delay = Delay::new();
    if HIL_START_DELAY_MS != 0 {
        uart.write(b"RFDBG_HIL_START_DELAY_BEGIN\r\n");
        delay.delay_millis(HIL_START_DELAY_MS);
        uart.write(b"RFDBG_HIL_START_DELAY_END\r\n");
    }

    let installed_storage = RADIO_STORAGE
        .install()
        .expect("install caller-owned radio storage");
    let scheduler_storage = RTOS_STORAGE
        .install(&RTOS_ARENA)
        .expect("install caller-owned scheduler storage");
    let rf_ready = RfPower::new(p.CMU, p.CLDO_CRG).enable(p.EFUSE, &mut delay);
    let (_cldo_crg, efuse) = rf_ready.into_parts();

    let runtime = hisi_rtos::ws63::start(
        hisi_rtos::ws63::Config {
            minimum_stack_size: NonZeroUsize::new(hisi_rf::ws63::SELECTED_MINIMUM_TASK_STACK_BYTES)
                .expect("selected profile minimum task stack is non-zero"),
            radio_task_policy: hisi_rtos::RunPolicy::Cooperative,
            max_scheduler_lock_duration: NonZeroU32::new(5_000).unwrap(),
        },
        hisi_rtos::ws63::Resources {
            timer: p.TIMER,
            software_interrupt: p.SYS_CTL1,
            storage: scheduler_storage,
            contract_violation: rtos_contract_violation,
            irqs: RtosIrqs::new(),
        },
    )
    .expect("start WS63 runtime");
    let main_task = runtime.handle().current_task().expect("adopted main task");
    runtime
        .handle()
        .set_task_run_policy(
            main_task,
            hisi_rtos::RunPolicy::Preemptive {
                time_slice: NonZeroU32::new(5).unwrap(),
            },
        )
        .expect("configure Embassy executor thread");

    uart.write(b"RF1_IMAGE_OK\r\n");

    #[cfg(feature = "wpa2")]
    let resources = installed_storage.resources(efuse, p.KM, p.SPACC, p.TRNG);
    #[cfg(feature = "wpa3")]
    let resources = installed_storage.resources(efuse, p.KM, p.SPACC, p.PKE, p.TRNG);

    let controller = match hisi_rf::ws63::init(config::radio_config(), resources) {
        Ok(controller) => controller,
        Err(error) => {
            write_diagnostic(uart, b"RF2_INIT_ERR:", error.diagnostic());
            write_heap_diagnostics(uart);
            halt()
        }
    };
    let parts = RADIO_PARTS.init(controller.split(RUNNER_BUDGET));
    start_executor(parts, uart)
}

#[inline(never)]
fn start_executor(parts: &'static mut RadioParts, uart: &'static Uart0) -> ! {
    let RadioParts { wifi, runner } = parts;
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(radio_runner(runner, uart).unwrap());
        spawner.spawn(connectivity(&mut wifi.controller, &mut wifi.device, uart).unwrap());
    })
}

#[embassy_executor::task]
async fn radio_runner(runner: &'static mut RadioRunner, uart: &'static Uart0) {
    loop {
        let ready = runner.wait_ready().await.expect("infallible WS63 wait");
        let started = monotonic_ms();
        let event = runner.run_once(ready).expect("incremental runner");
        uart.write(b"RFDBG_A5B_RUNNER_ELAPSED_MS value=0x");
        uart.write(&hex8(
            monotonic_ms().wrapping_sub(started).min(u32::MAX as u64) as u32,
        ));
        uart.write(b"\r\n");
        write_runner_event(uart, event);
    }
}

#[embassy_executor::task]
async fn connectivity(
    controller: &'static mut WifiController,
    device: &'static mut WifiDevice,
    uart: &'static Uart0,
) {
    let initialize_started = monotonic_ms();
    match with_timeout(INITIALIZE_WAIT_DEADLINE, controller.initialize()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            write_controller_error(uart, b"RF2_INIT_ERR:", error);
            halt()
        }
        Err(_) => {
            uart.write(b"RF2_INIT_ERR:timeout\r\n");
            halt()
        }
    }
    uart.write(b"RFDBG_A5B_INITIALIZE_OK elapsed_ms=0x");
    uart.write(&hex8(
        monotonic_ms()
            .wrapping_sub(initialize_started)
            .min(u32::MAX as u64) as u32,
    ));
    uart.write(b"\r\n");
    uart.write(b"RF2_INIT_OK ifname=hisi-rf\r\n");
    expect_event(uart, controller, ExpectedEvent::Initialized).await;
    #[cfg(feature = "diagnostic-disable-sta-pm")]
    match hisi_rf::ws63::disable_station_power_save_for_diagnostics() {
        Ok(()) => uart.write(b"RFDBG_STA_PM_DIAG phase=pre-association mode=off status=ok\r\n"),
        Err(hisi_rf::ws63::StationPowerSaveDiagnosticError::Vendor(status)) => {
            uart.write(
                b"RFDBG_STA_PM_DIAG phase=pre-association mode=off status=vendor-error code=0x",
            );
            uart.write(&hex8(status as u32));
            uart.write(b"\r\n");
            halt()
        }
        Err(hisi_rf::ws63::StationPowerSaveDiagnosticError::UnsupportedTarget) => {
            uart.write(
                b"RFDBG_STA_PM_DIAG phase=pre-association mode=off status=unsupported-target\r\n",
            );
            halt()
        }
    }

    let mut scan_results = [ScanResult::empty(); SCAN_RESULT_DEPTH];
    let mut retries = 0_u8;
    let scan_started = monotonic_ms();
    let outcome = loop {
        let result = match with_timeout(
            SCAN_WAIT_DEADLINE,
            controller.scan(ScanConfig::new(SCAN_OPERATION_TIMEOUT), &mut scan_results),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                write_scan_diagnostics(uart, controller, device, u32::from(retries) + 1);
                uart.write(b"RF3_SCAN_ERR reason=outer_timeout\r\n");
                halt()
            }
        };
        expect_scan_event(uart, controller, result).await;
        match result {
            Ok(outcome) => {
                if scan_results[..outcome.count]
                    .iter()
                    .any(|result| result.ssid.as_bytes() == TEST_SSID)
                {
                    break outcome;
                }
                if retries == 0 {
                    retries = 1;
                    Timer::after(Duration::from_millis(250)).await;
                    continue;
                }
                write_scan_inventory(uart, &scan_results[..outcome.count]);
                uart.write(b"RF5B_AP_NOT_FOUND\r\n");
                halt()
            }
            Err(error)
                if retries == 0
                    && error.diagnostic().code() == DiagnosticCode::OperationTimeout =>
            {
                write_scan_diagnostics(uart, controller, device, 1);
                retries = 1;
                Timer::after(Duration::from_millis(250)).await;
            }
            Err(error) => {
                write_scan_diagnostics(uart, controller, device, u32::from(retries) + 1);
                write_controller_error(uart, b"RF3_SCAN_ERR", error);
                halt()
            }
        }
    };
    uart.write(b"RFDBG_A5B_SCAN_OK elapsed_ms=0x");
    uart.write(&hex8(
        monotonic_ms()
            .wrapping_sub(scan_started)
            .min(u32::MAX as u64) as u32,
    ));
    uart.write(b" count=0x");
    uart.write(&hex8(outcome.count.min(u32::MAX as usize) as u32));
    uart.write(b" truncated=0x");
    uart.write(&hex8(u32::from(outcome.truncated)));
    uart.write(b"\r\n");
    uart.write(b"RF3_SCAN_OK count=0x");
    uart.write(&hex8(outcome.count.min(u32::MAX as usize) as u32));
    uart.write(b" truncated=0x");
    uart.write(&hex8(u32::from(outcome.truncated)));
    uart.write(b"\r\n");
    uart.write(b"A4_RADIO_EVENT kind=scan-completed\r\n");

    let result = scan_results[..outcome.count]
        .iter()
        .find(|result| result.ssid.as_bytes() == TEST_SSID)
        .expect("scan result checked above");
    let mut reconnects = 0_u32;
    loop {
        let Some(passphrase) = Passphrase::try_from_ascii(TEST_PASSPHRASE) else {
            uart.write(b"RF5B_CONNECT_ERR:invalid_credentials\r\n");
            halt()
        };
        #[cfg(feature = "wpa2")]
        let station = StationConfig::wpa2_personal(result, passphrase, CONNECT_OPERATION_TIMEOUT);
        #[cfg(feature = "wpa3")]
        let station = StationConfig::wpa3_personal(
            result,
            passphrase,
            SaePwe::Both,
            CONNECT_OPERATION_TIMEOUT,
        );
        let Some(station) = station else {
            uart.write(b"RF5B_CONNECT_ERR:security_mismatch\r\n");
            halt()
        };

        uart.write(b"RF5B_CONNECT_BEGIN reconnect=0x");
        uart.write(&hex8(reconnects));
        uart.write(b"\r\n");
        let connect_started = monotonic_ms();
        match with_timeout(CONNECT_WAIT_DEADLINE, controller.connect(station)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                write_controller_error(uart, b"RF5B_CONNECT_ERR:", error);
                write_a5b_evidence(uart, controller, device);
                halt()
            }
            Err(_) => {
                uart.write(b"RF5B_CONNECT_ERR:outer_timeout\r\n");
                halt()
            }
        }
        uart.write(b"RFDBG_A5B_CONNECT_OK elapsed_ms=0x");
        uart.write(&hex8(
            monotonic_ms()
                .wrapping_sub(connect_started)
                .min(u32::MAX as u64) as u32,
        ));
        uart.write(b" reconnect=0x");
        uart.write(&hex8(reconnects));
        uart.write(b"\r\n");
        #[cfg(feature = "wpa2")]
        uart.write(b"W2D_WPA2_CONNECT_OK\r\n");
        #[cfg(feature = "wpa3")]
        {
            uart.write(b"W2E_WPA3_CONNECT_OK pmf=required\r\n");
            match result.security {
                Security::Wpa3Personal => uart.write(b"W2E_AP_SECURITY mode=pure-wpa3\r\n"),
                Security::Wpa2Wpa3PersonalTransition => {
                    uart.write(b"W2E_AP_SECURITY mode=transition\r\n");
                }
                _ => {}
            }
        }
        expect_event(uart, controller, ExpectedEvent::Connected).await;
        write_a5b_evidence(uart, controller, device);
        match network_runner::run(uart, controller, device).await {
            network_runner::NetworkExit::Disconnected { reason } => {
                uart.write(b"A4_RECONNECT reason=disconnected code=0x");
                uart.write(&hex8(u32::from(reason)));
                uart.write(b"\r\n");
            }
            network_runner::NetworkExit::BackendFailed => {
                uart.write(b"A4_NET_ERR:backend-failed\r\n");
                write_a5b_evidence(uart, controller, device);
                halt()
            }
        }
        reconnects = reconnects.saturating_add(1);
        Timer::after(Duration::from_millis(250)).await;
    }
}

enum ExpectedEvent {
    Initialized,
    Connected,
}

async fn expect_scan_event(
    uart: &Uart0,
    controller: &mut WifiController,
    result: Result<ScanOutcome, WifiError>,
) {
    let event = next_event_or_halt(uart, controller).await;
    let matches = match (result, event) {
        (Ok(expected), WifiEvent::ScanCompleted { count, truncated })
            if count == expected.count && truncated == expected.truncated =>
        {
            true
        }
        (Err(WifiError::Backend(expected)), WifiEvent::Failed(actual)) if expected == actual => {
            true
        }
        (Err(WifiError::Protocol), WifiEvent::Failed(_)) => true,
        _ => false,
    };
    if !matches {
        write_unexpected_event(uart, event);
        halt()
    }
}

async fn expect_event(uart: &Uart0, controller: &mut WifiController, expected: ExpectedEvent) {
    let event = next_event_or_halt(uart, controller).await;
    let matches = match (expected, event) {
        (ExpectedEvent::Initialized, WifiEvent::Initialized) => {
            uart.write(b"A4_RADIO_EVENT kind=initialized\r\n");
            true
        }
        (ExpectedEvent::Connected, WifiEvent::Connected(_)) => {
            uart.write(b"A4_RADIO_EVENT kind=connected\r\n");
            true
        }
        _ => false,
    };
    if !matches {
        write_unexpected_event(uart, event);
        halt()
    }
}

async fn next_event_or_halt(uart: &Uart0, controller: &mut WifiController) -> WifiEvent {
    with_timeout(EVENT_WAIT_DEADLINE, controller.next_event())
        .await
        .unwrap_or_else(|_| {
            uart.write(b"RFDBG_A4_EVENT_ERR reason=timeout\r\n");
            halt()
        })
}

fn write_unexpected_event(uart: &Uart0, event: WifiEvent) {
    uart.write(b"RFDBG_A4_EVENT_ERR reason=unexpected kind=");
    match event {
        WifiEvent::Initialized => uart.write(b"initialized"),
        WifiEvent::ScanCompleted { .. } => uart.write(b"scan-completed"),
        WifiEvent::Connected(_) => uart.write(b"connected"),
        WifiEvent::Disconnected { .. } => uart.write(b"disconnected"),
        WifiEvent::Failed(_) => uart.write(b"failed"),
    }
    uart.write(b"\r\n");
}

fn write_a5b_evidence(uart: &Uart0, controller: &WifiController, device: &WifiDevice) {
    let snapshot = hisi_rf::ws63::diagnostics(controller, device);
    let event = snapshot.events;
    uart.write(b"RFDBG_A5B_EVENT pending=0x");
    uart.write(&hex8(event.pending.min(u32::MAX as usize) as u32));
    uart.write(b" high_water=0x");
    uart.write(&hex8(event.high_water.min(u32::MAX as usize) as u32));
    uart.write(b" dropped=0x");
    uart.write(&hex8(event.dropped));
    uart.write(b"\r\n");

    let control = snapshot.control;
    uart.write(b"RFDBG_A5B_CONTROL pending=0x");
    uart.write(&hex8(
        control.command_queue_pending.min(u32::MAX as usize) as u32
    ));
    uart.write(b" high_water=0x");
    uart.write(&hex8(
        control.command_queue_high_water.min(u32::MAX as usize) as u32,
    ));
    uart.write(b"\r\n");

    match snapshot.runner {
        RunnerDiagnosticsSnapshot::Incremental(value) => write_runner_diagnostics(uart, value),
        RunnerDiagnosticsSnapshot::Blocking(_) => {
            uart.write(b"RFDBG_A5B_RUNNER_ERR reason=wrong_profile\r\n");
        }
    }
    write_wait_diagnostics(uart, snapshot.wait);
    write_osal_diagnostics(uart);

    let blocking = snapshot.blocking_calls;
    uart.write(b"RFDBG_A5B_BLOCKING init_calls=0x");
    uart.write(&hex8(blocking.initialize.calls));
    uart.write(b" init_max_ms=0x");
    uart.write(&hex8(blocking.initialize.max_elapsed_ms));
    uart.write(b" scan_calls=0x");
    uart.write(&hex8(blocking.scan.calls));
    uart.write(b" poll_calls=0x");
    uart.write(&hex8(blocking.poll.calls));
    uart.write(b" internal_sleep=0x");
    uart.write(&hex8(blocking.internal_sleep_calls));
    uart.write(b" supplicant_poll=0x");
    uart.write(&hex8(blocking.supplicant_poll_calls));
    uart.write(b"\r\n");

    let frw_sync = blocking.frw_sync_post;
    uart.write(b"RFDBG_A5B_FRW_SYNC calls=0x");
    uart.write(&hex8(frw_sync.calls));
    uart.write(b" last_id=0x");
    uart.write(&hex8(frw_sync.last_msg_id));
    uart.write(b" last_timeout_ms=0x");
    uart.write(&hex8(frw_sync.last_timeout_ms));
    uart.write(b" last_elapsed_ms=0x");
    uart.write(&hex8(frw_sync.last_elapsed_ms));
    uart.write(b" last_ret=0x");
    uart.write(&hex8(frw_sync.last_result));
    uart.write(b" max_id=0x");
    uart.write(&hex8(frw_sync.max_msg_id));
    uart.write(b" max_elapsed_ms=0x");
    uart.write(&hex8(frw_sync.max_elapsed_ms));
    uart.write(b" wait_blocks=0x");
    uart.write(&hex8(frw_sync.last_wait_blocks));
    uart.write(b" wait_wakeups=0x");
    uart.write(&hex8(frw_sync.last_wait_wakeups));
    uart.write(b" wait_ready=0x");
    uart.write(&hex8(frw_sync.last_wait_ready_checks));
    uart.write(b"\r\n");

    write_rtos_task_diagnostics(uart);

    let association = hisi_rf::ws63::association_timing_diagnostics();
    uart.write(b"RFDBG_A5B_CONNECT_ASSOC_IOCTL");
    for value in [
        association.first.calls,
        association.first.last_elapsed_ms,
        association.first.max_elapsed_ms,
        association.clear.calls,
        association.clear.last_elapsed_ms,
        association.clear.max_elapsed_ms,
        association.retry.calls,
        association.retry.last_elapsed_ms,
        association.retry.max_elapsed_ms,
        association.deauthenticate.calls,
        association.deauthenticate.last_elapsed_ms,
        association.deauthenticate.max_elapsed_ms,
    ] {
        uart.write(b" 0x");
        uart.write(&hex8(value));
    }
    uart.write(b"\r\nRFDBG_A5B_CONNECT_PROFILE_OK\r\n");
}

fn write_rtos_task_diagnostics(uart: &Uart0) {
    let scheduler = hisi_rtos::diagnostics();
    uart.write(b"RFDBG_A5B_SCHED ready_owner_err=0x");
    uart.write(&hex8(u32::from(scheduler.ready_ownership_violations)));
    uart.write(b" ready_dup=0x");
    uart.write(&hex8(u32::from(
        scheduler.ready_queue_duplicate_memberships,
    )));
    uart.write(b" ready_wrong_bucket=0x");
    uart.write(&hex8(u32::from(scheduler.ready_queue_wrong_priorities)));
    uart.write(b" ready_bad_link=0x");
    uart.write(&hex8(u32::from(scheduler.ready_queue_invalid_links)));
    uart.write(b" detached_prio_mut=0x");
    uart.write(&hex8(scheduler.detached_pending_priority_mutations));
    uart.write(b" detached_policy_mut=0x");
    uart.write(&hex8(scheduler.detached_pending_policy_mutations));
    uart.write(b"\r\n");

    let mut tasks = [hisi_rtos::TaskDiagnostic::default(); 17];
    let count = hisi_rtos::task_diagnostics(&mut tasks);
    for task in &tasks[..count] {
        uart.write(b"RFDBG_A5B_TASK id=0x");
        uart.write(&hex8(task.task as u32));
        uart.write(b" state=");
        uart.write(task_state_name(task.state));
        uart.write(b" entry=0x");
        uart.write(&hex8(task.entry as u32));
        uart.write(b" base_prio=0x");
        uart.write(&hex8(u32::from(task.base_priority)));
        uart.write(b" prio=0x");
        uart.write(&hex8(u32::from(task.priority)));
        uart.write(b" ready_queued=");
        uart.write(if task.ready_queued { b"1" } else { b"0" });
        uart.write(b" pending_target=");
        uart.write(if task.pending_switch_target {
            b"1"
        } else {
            b"0"
        });
        uart.write(b" ready_bucket=0x");
        uart.write(&hex8(u32::from(task.ready_queue_bucket)));
        uart.write(b" ready_memberships=0x");
        uart.write(&hex8(u32::from(task.ready_queue_memberships)));
        uart.write(b" wait_sem=0x");
        uart.write(&hex8(task.waiting_sem as u32));
        uart.write(b" wake_at=0x");
        uart.write(&hex8(task.wake_at as u32));
        uart.write(b" dispatches=0x");
        uart.write(&hex8(task.dispatches));
        uart.write(b" max_ready_ms=0x");
        uart.write(&hex8(task.max_ready_latency_ms as u32));
        uart.write(b" max_run_ms=0x");
        uart.write(&hex8(
            task.max_continuous_run_ms.min(u64::from(u32::MAX)) as u32
        ));
        uart.write(b" max_lock_ms=0x");
        uart.write(&hex8(
            task.max_scheduler_lock_ms.min(u64::from(u32::MAX)) as u32
        ));
        uart.write(b"\r\n");
    }
}

pub(crate) fn write_osal_diagnostics(uart: &Uart0) {
    let mut waits = [hisi_rf::ws63::OsalWaitDiagnostic::default(); 16];
    let wait_count = hisi_rf::ws63::osal_wait_diagnostics(&mut waits);
    for wait in &waits[..wait_count] {
        uart.write(b"RFDBG_A5B_OSAL_WAIT wait=0x");
        uart.write(&hex8(wait.wait as u32));
        uart.write(b" sem=0x");
        uart.write(&hex8(wait.semaphore as u32));
        uart.write(b" pred=0x");
        uart.write(&hex8(wait.predicate as u32));
        uart.write(b" param=0x");
        uart.write(&hex8(wait.parameter as u32));
        uart.write(b" pred_now=0x");
        uart.write(&hex8(wait.predicate_result as u32));
        uart.write(b" blocks=0x");
        uart.write(&hex8(wait.blocks));
        uart.write(b" wakeups=0x");
        uart.write(&hex8(wait.wakeups));
        uart.write(b" ready=0x");
        uart.write(&hex8(wait.ready_checks));
        uart.write(b" wait_task=0x");
        uart.write(&hex8(wait.last_wait_task as u32));
        uart.write(b" wake_task=0x");
        uart.write(&hex8(wait.last_wake_task as u32));
        uart.write(b" wait_ra=0x");
        uart.write(&hex8(wait.last_wait_caller as u32));
        uart.write(b" wake_ra=0x");
        uart.write(&hex8(wait.last_wake_caller as u32));
        uart.write(b"\r\n");
    }

    let mut events = [hisi_rf::ws63::OsalEventDiagnostic::default(); 16];
    let event_count = hisi_rf::ws63::osal_event_diagnostics(&mut events);
    for event in &events[..event_count] {
        uart.write(b"RFDBG_A5B_OSAL_EVENT event=0x");
        uart.write(&hex8(event.event as u32));
        uart.write(b" bits=0x");
        uart.write(&hex8(event.bits));
        uart.write(b" reads=0x");
        uart.write(&hex8(event.reads));
        uart.write(b" writes=0x");
        uart.write(&hex8(event.writes));
        uart.write(b" matches=0x");
        uart.write(&hex8(event.matches));
        uart.write(b" read_mask=0x");
        uart.write(&hex8(event.last_read_mask));
        uart.write(b" write_mask=0x");
        uart.write(&hex8(event.last_write_mask));
        uart.write(b" mode=0x");
        uart.write(&hex8(event.last_mode));
        uart.write(b"\r\n");
    }
}

const fn task_state_name(state: hisi_rtos::TaskState) -> &'static [u8] {
    match state {
        hisi_rtos::TaskState::Free => b"free",
        hisi_rtos::TaskState::Ready => b"ready",
        hisi_rtos::TaskState::Running => b"running",
        hisi_rtos::TaskState::Blocked => b"blocked",
        hisi_rtos::TaskState::Sleeping => b"sleeping",
        hisi_rtos::TaskState::Throttled => b"throttled",
    }
}

fn write_runner_diagnostics(uart: &Uart0, diagnostics: hisi_rf::IncrementalRunnerDiagnostics) {
    uart.write(b"RFDBG_A5B_RUNNER run=0x");
    uart.write(&hex8(diagnostics.run_once_calls));
    uart.write(b" waits=0x");
    uart.write(&hex8(diagnostics.wait_ready_calls));
    uart.write(b" wake=0x");
    uart.write(&hex8(diagnostics.wait_ready_completions));
    uart.write(b" immediate=0x");
    uart.write(&hex8(diagnostics.immediate_ready_completions));
    uart.write(b" operations=0x");
    uart.write(&hex8(diagnostics.operations_started));
    uart.write(b" completed=0x");
    uart.write(&hex8(diagnostics.operations_completed));
    uart.write(b" pending=0x");
    uart.write(&hex8(diagnostics.pending_polls));
    uart.write(b" exhausted=0x");
    uart.write(&hex8(diagnostics.budget_exhaustions));
    uart.write(b" errors=0x");
    uart.write(&hex8(
        diagnostics
            .driver_errors
            .saturating_add(diagnostics.protocol_errors)
            .saturating_add(diagnostics.wait_ready_errors),
    ));
    uart.write(b"\r\n");
}

fn write_wait_diagnostics(uart: &Uart0, diagnostics: WaitDiagnosticsSnapshot) {
    uart.write(b"RFDBG_A5B_WAIT backend=0x");
    uart.write(&hex8(diagnostics.backend_signals));
    uart.write(b" l2=0x");
    uart.write(&hex8(diagnostics.l2_rx_signals));
    uart.write(b" waker=0x");
    uart.write(&hex8(diagnostics.waker_notifications));
    uart.write(b" polls=0x");
    uart.write(&hex8(diagnostics.poll_calls));
    uart.write(b" pending=0x");
    uart.write(&hex8(diagnostics.pending_polls));
    uart.write(b" ready=0x");
    uart.write(&hex8(diagnostics.ready_polls));
    uart.write(b" timer=0x");
    uart.write(&hex8(diagnostics.timer_ready_polls));
    uart.write(b"\r\n");
}

fn write_runner_event(uart: &Uart0, event: IncrementalDriverEvent) {
    uart.write(b"RFDBG_A5B_RUNNER_EVENT kind=");
    match event {
        IncrementalDriverEvent::Idle => uart.write(b"idle"),
        IncrementalDriverEvent::Started { .. } => uart.write(b"started"),
        IncrementalDriverEvent::Waiting { .. } => uart.write(b"waiting"),
        IncrementalDriverEvent::Pending { .. } => uart.write(b"pending"),
        IncrementalDriverEvent::BudgetExhausted { .. } => uart.write(b"budget-exhausted"),
        IncrementalDriverEvent::CancelRequested { .. } => uart.write(b"cancel-requested"),
        IncrementalDriverEvent::Completed { .. } => uart.write(b"completed"),
        IncrementalDriverEvent::Cancelled { .. } => uart.write(b"cancelled"),
        IncrementalDriverEvent::Failed { .. } => uart.write(b"failed"),
    }
    uart.write(b"\r\n");
}

fn write_scan_diagnostics(
    uart: &Uart0,
    controller: &WifiController,
    device: &WifiDevice,
    attempt: u32,
) {
    let scan = hisi_rf::ws63::diagnostics(controller, device).scan;
    uart.write(b"RFDBG_SCAN_DIAG attempt=0x");
    uart.write(&hex8(attempt));
    uart.write(b" native_starts=0x");
    uart.write(&hex8(scan.native_starts));
    uart.write(b" native_results=0x");
    uart.write(&hex8(scan.native_results));
    uart.write(b" native_done=0x");
    uart.write(&hex8(scan.native_done));
    uart.write(b" native_active=0x");
    uart.write(&hex8(u32::from(scan.native_active)));
    uart.write(b" queue_pending=0x");
    uart.write(&hex8(u32::from(scan.queue_pending)));
    uart.write(b" queue_dropped=0x");
    uart.write(&hex8(scan.queue_dropped));
    uart.write(b" native_start_ms=0x");
    uart.write(&hex8(scan.native_start_ms));
    uart.write(b" native_observed_ms=0x");
    uart.write(&hex8(scan.native_done_ms));
    uart.write(b" driver_active=0x");
    uart.write(&hex8(u32::from(scan.driver_active)));
    uart.write(b" driver_done=0x");
    uart.write(&hex8(u32::from(scan.driver_done)));
    uart.write(b" driver_results=0x");
    uart.write(&hex8(scan.driver_results));
    uart.write(b" driver_status=0x");
    uart.write(&hex8(scan.driver_status));
    uart.write(b" driver_observed_ms=0x");
    uart.write(&hex8(scan.driver_done_ms));
    uart.write(b"\r\n");
}

fn write_scan_inventory(uart: &Uart0, results: &[ScanResult]) {
    uart.write(b"RFDBG_SCAN_TARGET len=0x");
    uart.write(&hex8(TEST_SSID.len() as u32));
    uart.write(b" hash=0x");
    uart.write(&hex8(fnv1a32(TEST_SSID)));
    uart.write(b"\r\n");

    for (index, result) in results.iter().enumerate() {
        uart.write(b"RFDBG_SCAN_RESULT index=0x");
        uart.write(&hex8(index as u32));
        uart.write(b" len=0x");
        uart.write(&hex8(result.ssid.as_bytes().len() as u32));
        uart.write(b" hash=0x");
        uart.write(&hex8(fnv1a32(result.ssid.as_bytes())));
        uart.write(b" channel=0x");
        uart.write(&hex8(u32::from(result.channel)));
        uart.write(b" security=");
        uart.write(match result.security {
            Security::Open => b"open",
            Security::Wpa2Personal => b"wpa2",
            Security::Wpa3Personal => b"wpa3",
            Security::Wpa2Wpa3PersonalTransition => b"wpa2-wpa3",
            Security::OtherProtected => b"other",
        });
        uart.write(b"\r\n");
    }
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn write_controller_error(uart: &Uart0, prefix: &[u8], error: hisi_rf::Error) {
    write_diagnostic(uart, prefix, error.diagnostic());
    write_driver_event_diagnostics(uart);
    write_supplicant_event_diagnostics(uart);
}

fn write_driver_event_diagnostics(uart: &Uart0) {
    let values = hisi_rf::ws63::upstream_supplicant_driver_event_diagnostic_snapshot();
    uart.write(b"RFDBG_DRIVER_EVENT calls=0x");
    uart.write(&hex8(values[0]));
    uart.write(b" last_kind=0x");
    uart.write(&hex8(values[1]));
    uart.write(b" last_length=0x");
    uart.write(&hex8(values[2]));
    uart.write(b" connect_calls=0x");
    uart.write(&hex8(values[3]));
    uart.write(b" connect_reject=0x");
    uart.write(&hex8(values[4]));
    uart.write(b" connect_queued=0x");
    uart.write(&hex8(values[5]));
    uart.write(b"\r\n");
}

fn write_supplicant_event_diagnostics(uart: &Uart0) {
    let event = hisi_rf::ws63::upstream_supplicant_event_diagnostic_snapshot();
    uart.write(b"RFDBG_SUPPLICANT_EVENT");
    for value in event {
        uart.write(b" 0x");
        uart.write(&hex8(value));
    }
    uart.write(b"\r\n");

    let eapol = hisi_rf::ws63::upstream_supplicant_eapol_diagnostic_snapshot();
    uart.write(b"RFDBG_SUPPLICANT_EAPOL");
    for value in eapol {
        uart.write(b" 0x");
        uart.write(&hex8(value));
    }
    uart.write(b"\r\n");
}

fn write_diagnostic(uart: &Uart0, prefix: &[u8], diagnostic: hisi_rf::Diagnostic) {
    uart.write(prefix);
    uart.write(b" code=");
    uart.write(diagnostic.code().as_str().as_bytes());
    uart.write(b" stage=");
    uart.write(diagnostic.stage().as_str().as_bytes());
    if let Some(code) = diagnostic.backend_code() {
        uart.write(b" backend=0x");
        uart.write(&hex8(code));
    }
    let trace = diagnostic.trace();
    for index in 0..trace.len() {
        let Some(entry) = trace.get(index) else {
            continue;
        };
        uart.write(b" ");
        uart.write(entry.kind().as_str().as_bytes());
        uart.write(b"=0x");
        uart.write(&hex8(entry.value()));
    }
    if trace.is_truncated() {
        uart.write(b" trace_truncated=1");
    }
    uart.write(b"\r\n");
}

pub(crate) fn write_heap_diagnostics(uart: &Uart0) {
    let resources = RADIO_STORAGE.report();
    uart.write(b"RFDBG_RESOURCE schema=");
    uart.write(resources.schema.as_bytes());
    uart.write(b" revision=");
    uart.write(resources.profile_revision.as_bytes());
    uart.write(b" runtime_arena=0x");
    uart.write(&hex8(resources.runtime_arena_bytes.unwrap_or(0) as u32));
    uart.write(b" rf_arena=0x");
    uart.write(&hex8(resources.shared_rf_arena_bytes.unwrap_or(0) as u32));
    uart.write(b" calibrated=0x");
    uart.write(&hex8(u32::from(resources.runtime_resources_calibrated)));
    uart.write(b"\r\n");

    let scheduler = RTOS_STORAGE.metrics();
    let radio = hisi_rf::ws63::rf_heap_metrics();
    uart.write(b"RFDBG_HEAP rtos_arena=0x");
    uart.write(&hex8(scheduler.arena_bytes as u32));
    uart.write(b" rtos_used=0x");
    uart.write(&hex8(scheduler.used_bytes as u32));
    uart.write(b" rtos_free=0x");
    uart.write(&hex8(scheduler.free_bytes as u32));
    uart.write(b" rtos_peak=0x");
    uart.write(&hex8(scheduler.peak_used_bytes as u32));
    uart.write(b" rtos_allocs=0x");
    uart.write(&hex8(scheduler.allocation_attempts as u32));
    uart.write(b" rtos_failures=0x");
    uart.write(&hex8(scheduler.allocation_failures as u32));
    uart.write(b" rf_arena=0x");
    uart.write(&hex8(radio.arena_bytes as u32));
    uart.write(b" rf_used=0x");
    uart.write(&hex8(radio.used_bytes as u32));
    uart.write(b" rf_free=0x");
    uart.write(&hex8(radio.free_bytes as u32));
    uart.write(b" rf_peak=0x");
    uart.write(&hex8(radio.peak_used_bytes as u32));
    uart.write(b" rf_failures=0x");
    uart.write(&hex8(radio.allocation_failures as u32));
    uart.write(b"\r\n");
}

pub(crate) fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn rtos_contract_violation(_violation: hisi_rtos::ContractViolation) -> ! {
    panic!("hisi-rtos scheduler contract violation")
}

pub(crate) fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

pub(crate) fn hex8(value: u32) -> [u8; 8] {
    let mut output = [0_u8; 8];
    for (index, digit) in output.iter_mut().enumerate() {
        let nibble = ((value >> ((7 - index) * 4)) & 0xf) as u8;
        *digit = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
    }
    output
}

pub(crate) fn write_ipv4(uart: &Uart0, octets: [u8; 4]) {
    for (index, octet) in octets.iter().enumerate() {
        if index != 0 {
            uart.write(b".");
        }
        let hundreds = octet / 100;
        let tens = (octet % 100) / 10;
        let ones = octet % 10;
        if hundreds != 0 {
            uart.write(&[b'0' + hundreds]);
        }
        if hundreds != 0 || tens != 0 {
            uart.write(&[b'0' + tens]);
        }
        uart.write(&[b'0' + ones]);
    }
}
