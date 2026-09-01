//! Fixed, non-production SoftAP for two-board WS63 HIL.

#![no_main]
#![no_std]

#[cfg(any(
    all(feature = "wpa2", feature = "wpa3"),
    not(any(feature = "wpa2", feature = "wpa3"))
))]
compile_error!("select exactly one SoftAP security profile: `wpa2` or `wpa3`");

#[cfg(all(
    any(feature = "ble-coexistence", feature = "sle-coexistence"),
    not(feature = "wpa2")
))]
compile_error!("the U7 coexistence fixture currently requires the WPA2 SoftAP profile");

#[cfg(all(feature = "ble-coexistence", feature = "sle-coexistence"))]
compile_error!("select exactly one coexistence activity: `ble-coexistence` or `sle-coexistence`");

#[path = "../../hil_wifi_config.rs"]
mod config;
mod network;

use core::num::NonZeroU32;
#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
use core::num::NonZeroUsize;

use hisi_hal::Peripherals;
use hisi_hal::delay::Delay;
use hisi_hal::interrupt;
use hisi_hal::rf_power::RfPower;
#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
use hisi_hal::software_interrupt::SoftwareInterrupt0;
use hisi_hal::time::Instant;
#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
use hisi_hal::timer::TimerAlarm0;
use hisi_hal::uart::{Config as UartConfig, Uart, UartClock};
use hisi_hal::wdt::Watchdog;
use hisi_panic_handler as _;
#[cfg(feature = "ble-coexistence")]
use hisi_rf::ws63::__coexistence::WifiWpa2AccessPointBleCoexistence;
#[cfg(feature = "sle-coexistence")]
use hisi_rf::ws63::__coexistence::WifiWpa2AccessPointSleCoexistence;
#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
use hisi_rf::ws63::__coexistence::{Profile, RadioArenaStorage, RadioStorage, Storage};
#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
use hisi_rf::ws63::{
    AccessPointConfig, AccessPointResources, InstalledAccessPointStorage,
    declare_access_point_storage,
};
use hisi_riscv_rt::entry;

#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
declare_access_point_storage!(static RADIO_STORAGE);

#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
const COEX_EVENTS: usize = 8;
#[cfg(feature = "ble-coexistence")]
type CoexProfile = WifiWpa2AccessPointBleCoexistence;
#[cfg(feature = "sle-coexistence")]
type CoexProfile = WifiWpa2AccessPointSleCoexistence;
#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
static RADIO_STORAGE: RadioStorage<
    CoexProfile,
    COEX_EVENTS,
    { <CoexProfile as Profile>::RF_ARENA_BYTES },
> = {
    static CONTROL: Storage<CoexProfile, COEX_EVENTS> = Storage::new();
    #[cfg_attr(target_arch = "riscv32", unsafe(link_section = ".hisi.shared-arena"))]
    static ARENA: RadioArenaStorage<{ <CoexProfile as Profile>::RF_ARENA_BYTES }> =
        RadioArenaStorage::new();
    RadioStorage::from_parts(&CONTROL, &ARENA)
};
#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
static RTOS_STORAGE: hisi_rtos::SchedulerStorage<15> = hisi_rtos::SchedulerStorage::new();
#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
#[cfg_attr(target_arch = "riscv32", unsafe(link_section = ".hisi.shared-arena"))]
static RTOS_ARENA: hisi_rtos::SchedulerArena<{ <CoexProfile as Profile>::RUNTIME_ARENA_BYTES }> =
    hisi_rtos::SchedulerArena::new();

#[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
hisi_rtos::bind_interrupts!(struct RtosIrqs {
    TIMER_INT0 => hisi_rtos::ws63::TimerInterrupt;
    SOFT_INT0 => hisi_rtos::ws63::SoftwareInterrupt;
});

#[entry]
fn main() -> ! {
    let p = Peripherals::take().expect("peripherals already taken");
    let uart = Uart::new_uart0(
        p.UART0,
        UartConfig {
            clock: UartClock::Boot,
            ..UartConfig::default()
        },
    );
    Watchdog::new(p.WDT).disable();
    uart.write(b"\r\nRFDBG_SOFTAP_BEGIN\r\n");

    let installed = RADIO_STORAGE.install().expect("install SoftAP storage");
    #[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
    let scheduler_storage = RTOS_STORAGE
        .install(&RTOS_ARENA)
        .expect("install SoftAP scheduler storage");
    uart.write(b"RFDBG_SOFTAP_STORAGE_OK\r\n");
    let mut delay = Delay::new();
    let rf_ready = RfPower::new(p.CMU, p.CLDO_CRG).enable(p.EFUSE, &mut delay);
    let (_cldo_crg, efuse) = rf_ready.into_parts();
    uart.write(b"RFDBG_SOFTAP_RF_POWER_OK\r\n");

    #[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
    let _timer = TimerAlarm0::new(p.TIMER);
    #[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    #[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
    let runtime = hisi_rtos::start_with_port(
        hisi_rtos::PortedConfig {
            radio_task_policy: hisi_rtos::RunPolicy::Cooperative,
            max_scheduler_lock_duration: NonZeroU32::new(5_000).unwrap(),
            ..hisi_rtos::PortedConfig::default()
        },
        hisi_rtos::Resources {
            allocate: rtos_allocate,
            deallocate: rtos_deallocate,
            monotonic_ms,
        },
        hisi_rtos::SchedulerPort {
            max_timer_delay: NonZeroU32::new(TimerAlarm0::MAX_DELAY_MS).unwrap(),
            arm_timer: TimerAlarm0::arm_millis,
            disarm_timer: TimerAlarm0::disarm,
            pend_reschedule: SoftwareInterrupt0::pend_interrupt,
            contract_violation: rtos_contract_violation,
        },
    )
    .expect("start ported runtime");
    #[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
    let runtime = hisi_rtos::ws63::start(
        hisi_rtos::ws63::Config {
            minimum_stack_size: NonZeroUsize::new(CoexProfile::MINIMUM_TASK_STACK_BYTES)
                .expect("profile minimum task stack"),
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
    .expect("start typed WS63 runtime");
    uart.write(b"RFDBG_SOFTAP_RTOS_OK\r\n");
    #[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
    let runtime_handle = &runtime;
    #[cfg(any(feature = "ble-coexistence", feature = "sle-coexistence"))]
    let runtime_handle = runtime.handle();
    let main_task = runtime_handle
        .current_task()
        .expect("adopted SoftAP main task");
    runtime_handle
        .set_task_run_policy(
            main_task,
            hisi_rtos::RunPolicy::Preemptive {
                time_slice: NonZeroU32::new(5).unwrap(),
            },
        )
        .expect("configure SoftAP application thread");
    unsafe { interrupt::enable_global() };
    uart.write(b"RFDBG_SOFTAP_IRQ_OK\r\n");

    #[cfg(all(
        feature = "wpa2",
        not(any(feature = "ble-coexistence", feature = "sle-coexistence"))
    ))]
    let resources = AccessPointResources::new(efuse, p.KM, p.SPACC, p.TRNG, installed);
    #[cfg(feature = "wpa3")]
    let resources = AccessPointResources::new(efuse, p.KM, p.SPACC, p.PKE, p.TRNG, installed);
    #[cfg(all(
        feature = "wpa2",
        not(any(feature = "ble-coexistence", feature = "sle-coexistence"))
    ))]
    let config =
        AccessPointConfig::wpa2_personal(config::SSID, config::PASSPHRASE, config::CHANNEL);
    #[cfg(feature = "wpa3")]
    let config = AccessPointConfig::wpa3_sae(config::SSID, config::PASSPHRASE, config::CHANNEL);
    uart.write(b"RFDBG_SOFTAP_INIT_BEGIN\r\n");
    #[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
    let mut access_point =
        hisi_rf::ws63::init_access_point(config, resources).expect("start native SoftAP");
    #[cfg(feature = "ble-coexistence")]
    let (mut access_point, ble) = {
        let (control, arena) = installed.into_init_parts();
        let resources = hisi_rf::ws63::__coexistence::Resources::<CoexProfile>::coexistence(
            efuse, p.KM, p.SPACC, p.PKE, p.TRNG, arena,
        );
        let config = hisi_rf::ws63::AccessPointConfig::wpa2_personal(
            config::SSID,
            config::PASSPHRASE,
            config::CHANNEL,
        );
        hisi_rf::ws63::__coexistence::init_access_point_ble_coexistence(config, resources, control)
            .expect("start native SoftAP plus BLE")
            .split()
    };
    #[cfg(feature = "sle-coexistence")]
    let (mut access_point, sle) = {
        let (control, arena) = installed.into_init_parts();
        let resources = hisi_rf::ws63::__coexistence::Resources::<CoexProfile>::coexistence(
            efuse, p.KM, p.SPACC, p.PKE, p.TRNG, arena,
        );
        let config = hisi_rf::ws63::AccessPointConfig::wpa2_personal(
            config::SSID,
            config::PASSPHRASE,
            config::CHANNEL,
        );
        hisi_rf::ws63::__coexistence::init_access_point_sle_coexistence(config, resources, control)
            .expect("start native SoftAP plus SLE")
            .split()
    };
    uart.write(b"RFDBG_SOFTAP_READY\r\n");
    let network_device = access_point
        .take_network_device()
        .expect("take SoftAP network device");
    #[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
    network::run(access_point, network_device, &uart);
    #[cfg(feature = "ble-coexistence")]
    network::run(access_point, network_device, ble, &uart);
    #[cfg(feature = "sle-coexistence")]
    network::run(access_point, network_device, sle, &uart)
}

fn write_diagnostics(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    diagnostics: hisi_rf::ws63::AccessPointDiagnostics,
) {
    uart.write(b"RFDBG_SOFTAP_STATE event=");
    uart.write(&hex8(diagnostics.events));
    uart.write(b" last=");
    uart.write(&hex8(diagnostics.last_event as u32));
    uart.write(b" len=");
    uart.write(&hex8(diagnostics.last_event_length));
    uart.write(b" invalid=");
    uart.write(&hex8(diagnostics.invalid_events));
    uart.write(b" mgmt_q=");
    uart.write(&hex8(diagnostics.management_queued));
    uart.write(b" mgmt_drop=");
    uart.write(&hex8(diagnostics.management_dropped));
    uart.write(b" mgmt_feed=");
    uart.write(&hex8(diagnostics.management_fed));
    uart.write(b" mgmt_feed_err=");
    uart.write(&hex8(diagnostics.management_feed_errors));
    uart.write(b" sta_assoc=");
    uart.write(&hex8(diagnostics.stations_associated));
    uart.write(b" sta_disassoc=");
    uart.write(&hex8(diagnostics.stations_disassociated));
    uart.write(b" sta_err=");
    uart.write(&hex8(diagnostics.station_feed_errors));
    uart.write(b" mgmt_tx=");
    uart.write(&hex8(diagnostics.management_transmits));
    uart.write(b" mgmt_tx_status=");
    uart.write(&hex8(diagnostics.last_management_status as u32));
    uart.write(b" eapol_poll=");
    uart.write(&hex8(diagnostics.eapol_polls));
    uart.write(b" eapol_rx=");
    uart.write(&hex8(diagnostics.eapol_received));
    uart.write(b" eapol_feed=");
    uart.write(&hex8(diagnostics.eapol_fed));
    uart.write(b" eapol_err=");
    uart.write(&hex8(diagnostics.eapol_errors));
    uart.write(b" eapol_tx=");
    uart.write(&hex8(diagnostics.eapol_transmits));
    uart.write(b" eapol_tx_status=");
    uart.write(&hex8(diagnostics.last_eapol_status as u32));
    uart.write(b" key=");
    uart.write(&hex8(diagnostics.key_installs));
    uart.write(b" key_status=");
    uart.write(&hex8(diagnostics.last_key_status as u32));
    #[cfg(feature = "data-path-diagnostics")]
    {
        uart.write(b" data_tx=");
        uart.write(&hex8(diagnostics.data_tx_frames));
        uart.write(b" data_tx_failed=");
        uart.write(&hex8(diagnostics.data_tx_failed));
        uart.write(b" data_vendor_tx=");
        uart.write(&hex8(diagnostics.data_vendor_tx_frames));
        uart.write(b" pbuf_ref=");
        uart.write(&hex8(diagnostics.data_tx_reference_diagnostics[0]));
        uart.write(b" tx_ref=");
        uart.write(&hex8(diagnostics.data_tx_reference_diagnostics[1]));
        uart.write(b" tx_no_ref=");
        uart.write(&hex8(diagnostics.data_tx_reference_diagnostics[2]));
        uart.write(b" data_tx_complete=");
        uart.write(&hex8(diagnostics.data_tx_completions));
        uart.write(b" data_tx_trace_total=");
        uart.write(&hex8(diagnostics.data_tx_completion_trace.0));
        uart.write(b" data_tx_trace=");
        for entry in diagnostics.data_tx_completion_trace.1 {
            uart.write(&hex8(entry));
            uart.write(b",");
        }
        uart.write(b" data_tx_pn_trace=");
        for packet_number in diagnostics.data_tx_completion_trace.2 {
            uart.write(&hex8(packet_number));
            uart.write(b",");
        }
        uart.write(b" data_tx_submit_total=");
        uart.write(&hex8(diagnostics.data_tx_timeline.submission_total));
        uart.write(b" data_tx_completion_total=");
        uart.write(&hex8(diagnostics.data_tx_timeline.completion_total));
        uart.write(b" data_tx_status=");
        for status in diagnostics.data_tx_completion_status {
            uart.write(&hex8(status));
            uart.write(b",");
        }
        uart.write(b" data_dmac_rx=");
        uart.write(&hex8(diagnostics.data_dmac_rx_prepares));
        uart.write(b" data_hmac_event=");
        uart.write(&hex8(diagnostics.data_hmac_rx_event_calls));
        uart.write(b" data_hmac_msg=");
        uart.write(&hex8(diagnostics.data_hmac_rx_msg_calls));
        uart.write(b" data_hmac_rx=");
        uart.write(&hex8(diagnostics.data_hmac_rx_calls));
        uart.write(b" data_vendor_rx=");
        uart.write(&hex8(diagnostics.data_vendor_rx_frames));
        uart.write(b" hmac_tx_calls=");
        uart.write(&hex8(diagnostics.data_hmac_tx.0));
        uart.write(b" hmac_tx_last=");
        uart.write(&hex8(diagnostics.data_hmac_tx.1));
        uart.write(b" hmac_tx_status=");
        for status in diagnostics.data_hmac_tx.2 {
            uart.write(&hex8(status));
            uart.write(b",");
        }
        uart.write(b" hmac_process_calls=");
        uart.write(&hex8(diagnostics.data_hmac_tx_process.0));
        uart.write(b" hmac_process_last=");
        uart.write(&hex8(diagnostics.data_hmac_tx_process.1));
        uart.write(b" hmac_process_status=");
        for status in diagnostics.data_hmac_tx_process.2 {
            uart.write(&hex8(status));
            uart.write(b",");
        }
        uart.write(b" hmac_send_calls=");
        uart.write(&hex8(diagnostics.data_hmac_tx_data_send[0]));
        uart.write(b" hmac_send_returns=");
        uart.write(&hex8(diagnostics.data_hmac_tx_data_send[1]));
        uart.write(b" frw_send_calls=");
        uart.write(&hex8(diagnostics.data_frw_hmac_send.0));
        uart.write(b" frw_send_last=");
        uart.write(&hex8(diagnostics.data_frw_hmac_send.1));
        uart.write(b" frw_send_status=");
        for status in diagnostics.data_frw_hmac_send.2 {
            uart.write(&hex8(status));
            uart.write(b",");
        }
        uart.write(b" dmac_event_calls=");
        uart.write(&hex8(diagnostics.data_dmac_tx_event.0));
        uart.write(b" dmac_event_last=");
        uart.write(&hex8(diagnostics.data_dmac_tx_event.1));
        uart.write(b" dmac_event_status=");
        for status in diagnostics.data_dmac_tx_event.2 {
            uart.write(&hex8(status));
            uart.write(b",");
        }
        uart.write(b" dmac_sw_queues=");
        for queue in diagnostics.data_dmac_tx_queues.0 {
            uart.write(&hex8(queue));
            uart.write(b",");
        }
        uart.write(b" dmac_hw_queues=");
        for queue in diagnostics.data_dmac_tx_queues.1 {
            uart.write(&hex8(queue));
            uart.write(b",");
        }
        uart.write(b" dmac_mac_q_status=");
        uart.write(&hex8(diagnostics.data_dmac_tx_mac_queue_status[0]));
        uart.write(b" dmac_mac_ext_q_status=");
        uart.write(&hex8(diagnostics.data_dmac_tx_mac_queue_status[1]));
        uart.write(b" dmac_queue_snapshot_stage=");
        uart.write(&hex8(diagnostics.data_dmac_tx_queue_snapshot_metadata[0]));
        uart.write(b" dmac_schedule_hook=");
        uart.write(&hex8(diagnostics.data_dmac_tx_queue_snapshot_metadata[1]));
        uart.write(b" psm_found=");
        uart.write(&hex8(diagnostics.data_psm[0]));
        uart.write(b" psm_vap=");
        uart.write(&hex8(diagnostics.data_psm[1]));
        uart.write(b" psm_mode=");
        uart.write(&hex8(diagnostics.data_psm[2]));
        uart.write(b" psm_empty=");
        uart.write(&hex8(diagnostics.data_psm[3]));
        uart.write(b" psm_tid_mpdu=");
        uart.write(&hex8(diagnostics.data_psm[4]));
        uart.write(b" ccmp_replay=");
        uart.write(&hex8(diagnostics.mac_ccmp_replay_failures));
        uart.write(b" ccmp_mic=");
        uart.write(&hex8(diagnostics.mac_ccmp_mic_failures));
        uart.write(b" key_search_fail=");
        uart.write(&hex8(diagnostics.mac_key_search_failures));
        uart.write(b" irq45=");
        uart.write(&hex8(diagnostics.wlmac_irqs));
        uart.write(b" irq45_en_calls=");
        uart.write(&hex8(diagnostics.wlmac_irq_lifecycle[0]));
        uart.write(b" irq45_dis_calls=");
        uart.write(&hex8(diagnostics.wlmac_irq_lifecycle[1]));
        uart.write(b" irq45_clr_calls=");
        uart.write(&hex8(diagnostics.wlmac_irq_lifecycle[2]));
        uart.write(b" irq45_enabled=");
        uart.write(&hex8(diagnostics.wlmac_irq_lifecycle[4]));
        uart.write(b" irq45_pending=");
        uart.write(&hex8(diagnostics.wlmac_irq_lifecycle[5]));
        uart.write(b" mac_tx_hi=");
        uart.write(&hex8(diagnostics.mac_tx_high_priority_mpdu));
        uart.write(b" mac_tx_norm=");
        uart.write(&hex8(diagnostics.mac_tx_normal_priority_mpdu));
        uart.write(b" mac_tx_ampdu_mpdu=");
        uart.write(&hex8(diagnostics.mac_tx_mpdu_in_ampdu));
        uart.write(b" mac_tx_ampdu=");
        uart.write(&hex8(diagnostics.mac_tx_ampdu));
        uart.write(b" mac_tx_irq=");
        uart.write(&hex8(diagnostics.mac_tx_complete_interrupts));
    }
    uart.write(b"\r\n");
}

pub(crate) fn write_rtos_task_diagnostics(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>) {
    let scheduler = hisi_rtos::diagnostics();
    uart.write(b"RFDBG_SOFTAP_SCHED switches=0x");
    uart.write(&hex8(scheduler.context_switches));
    uart.write(b" irq_preempt=0x");
    uart.write(&hex8(scheduler.irq_preemptions));
    uart.write(b" ready=0x");
    uart.write(&hex8(u32::from(scheduler.ready_tasks)));
    uart.write(b" blocked=0x");
    uart.write(&hex8(u32::from(scheduler.blocked_tasks)));
    uart.write(b" lock_depth=0x");
    uart.write(&hex8(u32::from(scheduler.current_lock_depth)));
    uart.write(b" lock_overrun=0x");
    uart.write(&hex8(scheduler.scheduler_lock_overruns));
    uart.write(b" detached_prio_mut=0x");
    uart.write(&hex8(scheduler.detached_pending_priority_mutations));
    uart.write(b" detached_policy_mut=0x");
    uart.write(&hex8(scheduler.detached_pending_policy_mutations));
    uart.write(b" ready_owner_err=0x");
    uart.write(&hex8(u32::from(scheduler.ready_ownership_violations)));
    uart.write(b" ready_dup=0x");
    uart.write(&hex8(u32::from(
        scheduler.ready_queue_duplicate_memberships,
    )));
    uart.write(b" ready_wrong_bucket=0x");
    uart.write(&hex8(u32::from(scheduler.ready_queue_wrong_priorities)));
    uart.write(b" ready_bad_link=0x");
    uart.write(&hex8(u32::from(scheduler.ready_queue_invalid_links)));
    uart.write(b"\r\n");

    let mut tasks = [hisi_rtos::TaskDiagnostic::default(); 17];
    let count = hisi_rtos::task_diagnostics(&mut tasks);
    for task in &tasks[..count] {
        uart.write(b"RFDBG_SOFTAP_TASK id=0x");
        uart.write(&hex8(task.task as u32));
        uart.write(b" state=");
        uart.write(task_state_name(task.state));
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
        uart.write(b" entry=0x");
        uart.write(&hex8(task.entry as u32));
        uart.write(b" base_prio=0x");
        uart.write(&hex8(u32::from(task.base_priority)));
        uart.write(b" prio=0x");
        uart.write(&hex8(u32::from(task.priority)));
        uart.write(b" dispatches=0x");
        uart.write(&hex8(task.dispatches));
        uart.write(b" max_ready_ms=0x");
        uart.write(&hex8(
            task.max_ready_latency_ms.min(u64::from(u32::MAX)) as u32
        ));
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

    write_osal_diagnostics(uart);
}

fn write_osal_diagnostics(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>) {
    let mut waits = [hisi_rf::ws63::OsalWaitDiagnostic::default(); 16];
    let wait_count = hisi_rf::ws63::osal_wait_diagnostics(&mut waits);
    for wait in &waits[..wait_count] {
        uart.write(b"RFDBG_SOFTAP_OSAL_WAIT wait=0x");
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
        uart.write(b"RFDBG_SOFTAP_OSAL_EVENT event=0x");
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

fn hex8(value: u32) -> [u8; 8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = [0; 8];
    let mut index = 0;
    while index < output.len() {
        let shift = (output.len() - 1 - index) * 4;
        output[index] = HEX[((value >> shift) & 0xf) as usize];
        index += 1;
    }
    output
}

#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
#[unsafe(no_mangle)]
extern "C" fn TIMER_INT0() {
    TimerAlarm0::clear_interrupt();
    hisi_rtos::interrupt_enter();
    hisi_rtos::on_timer_interrupt();
    hisi_rtos::interrupt_exit();
}

#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
#[unsafe(no_mangle)]
extern "C" fn SOFT_INT0() {
    SoftwareInterrupt0::clear_interrupt();
    hisi_rtos::interrupt_enter();
    hisi_rtos::on_software_interrupt();
    hisi_rtos::interrupt_exit();
}

#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
unsafe fn rtos_allocate(size: usize) -> *mut u8 {
    unsafe {
        InstalledAccessPointStorage::<{ hisi_rf::ws63::ACCESS_POINT_ARENA_BYTES }>::allocate(size)
    }
}

#[cfg(not(any(feature = "ble-coexistence", feature = "sle-coexistence")))]
unsafe fn rtos_deallocate(pointer: *mut u8) {
    unsafe {
        InstalledAccessPointStorage::<{ hisi_rf::ws63::ACCESS_POINT_ARENA_BYTES }>::deallocate(
            pointer,
        )
    };
}

fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn rtos_contract_violation(_violation: hisi_rtos::ContractViolation) -> ! {
    panic!("hisi-rtos scheduler contract violation")
}
