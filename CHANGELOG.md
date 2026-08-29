# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- Updated the Wi-Fi STA and SoftAP fixtures to `hisi-rf 0.1.0-alpha.101`,
  closing the U7 release train over `hisi-rf-ws63 0.1.0-alpha.86`,
  `hisi-rf-rtos-driver 0.1.0-alpha.20`, and `hisi-rtos 0.1.0-alpha.25`.
  The selected named profiles now use the same admission and shared-platform
  versions that passed the fixed-image coexistence initialization matrices.

- Updated the Wi-Fi STA and SoftAP fixtures to `hisi-rf 0.1.0-alpha.97`,
  closing the release chain over `hisi-rf-ws63 0.1.0-alpha.83` and
  `ws63-radio-sys 0.1.0-alpha.21` with hardware-backed BLE Secure
  Connections key generation and ECDH.

- Updated the Wi-Fi STA and SoftAP fixtures to `hisi-rf 0.1.0-alpha.85`, whose
  release train consumes the published WS63 BLE B1 archive closure without
  exposing a BLE application API.

- Updated both Wi-Fi fixtures to `hisi-rf 0.1.0-alpha.84`, closing the
  release chain over `hisi-rf-ws63 0.1.0-alpha.72` and the normalized BLE
  archive contract in `ws63-radio-sys 0.1.0-alpha.12`.
- Updated both Wi-Fi fixtures to `hisi-rf 0.1.0-alpha.83`. The station
  example now selects the complete bounded runner through its named profile,
  without opting into implementation features, and uses the facade's neutral
  `RadioParts` / `RadioRunner` lifecycle names.
- Updated both Wi-Fi fixtures to `hisi-rtos 0.1.0-alpha.23`, so the release
  closure includes the verified switch-target ownership and linearized
  switch-away fixes used by the dual-board HIL path.

- Added SoftAP scheduler diagnostics for priority/policy mutations of a
  detached pending switch target, so HIL can distinguish a constructible RTOS
  ownership defect from a trigger observed on silicon.
- Updated both Wi-Fi fixtures to `hisi-rf 0.1.0-alpha.82` and added matching
  repository-owned WPA3-SAE AP/STA profiles. The WPA3 AP consumes the typed
  PKE resource and emits secret-free P-256 request/failure counters for HIL.
  Both examples depend only on the public `hisi-rf` facade; it selects the
  upstream authenticator or supplicant and the shared STA/AP radio arena.
- Updated both fixtures to `hisi-rtos 0.1.0-alpha.22`, which closes the
  on-silicon resumed switch-away race exposed by the SoftAP workload.
- Extended `wifi_softap` into a self-contained two-board HIL endpoint with a
  fixed local IPv4 address, DHCP lease service, and bounded UDP echo. The
  `wifi_connectivity` `dual-board-hil` feature now proves WPA2, DHCP, direct
  ARP, UDP echo, and lease renewal without a developer credential file.
- Tightened the local connectivity gate so the isolated dual-board fixture
  requires both direct ARP evidence and a sequence-checked UDP echo response;
  public DNS is skipped only when DHCP intentionally supplies no default route.
- Fixed `wifi_connectivity` scan retries so every completed scan consumes and
  validates its matching `WifiEvent`. A timed-out first attempt can no longer
  leave a stale failure ahead of the replacement scan's completion event.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.77` and
  `hisi-rtos 0.1.0-alpha.19`; vendor and incremental-worker stacks are admitted
  atomically before RF initialization, and counted backend/L2 wake delivery
  preserves queued EAPOL progress.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.74`. The WS63
  incremental composition now admits its seven vendor tasks and one Rust
  worker independently, matching the split reservation proven by init/scan
  HIL.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.73`. Its opt-in
  incremental runner now delegates synchronous vendor turns to the caller-owned
  budgeted RTOS worker, with the 8 KiB worker stack and bounded control state
  represented in the complete SRAM contract. The example remains the dual-board
  HIL calibration path before that worker can become a default profile.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.71`. Incremental
  operations now retain ownership after an uninterruptible backend turn
  exceeds its elapsed time grant, allowing later completion events to be
  attributed correctly while reporting budget exhaustion.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.60` and replaced its
  process-global runner/wait diagnostic cells with the facade-owned,
  task-split-safe unified snapshot.
- Extended the post-ping diagnostic marker through the v5 data-path
  chain: smoltcp TX, vendor bridge TX, DMAC completion, vendor/Rust RX, MAC
  receive counters, IRQ dispatches, and an explicit instrumentation capability
  mask.
- Updated `wifi_connectivity` to the v6 aggregate diagnostics in
  `hisi-rf 0.1.0-alpha.64`; the post-ping marker now reports the active WLMAC
  packed receive-filter control plus secret-free station-match and BSSID-programmed
  state.
- Updated `wifi_connectivity` to the v7 aggregate diagnostics in
  `hisi-rf 0.1.0-alpha.65`; scan timeout and retry boundaries now report
  secret-free native callback, event-queue, and vendor-driver scan state.
- Updated `wifi_connectivity` to the v8 aggregate diagnostics in
  `hisi-rf 0.1.0-alpha.66`; post-connect output now distinguishes ARP request,
  ARP reply, IPv4, and other Ethernet traffic in each direction. The legacy
  `RF5A_ARP_OK` marker now states that its evidence is an ICMP reply rather than
  pretending to be a direct neighbor-cache observation.
- Made the local connectivity gate explicit: DHCP plus at least one gateway
  reply is required, while AliDNS ICMP is emitted only as packet-loss
  observation and no longer decides HIL success.
- Added an opt-in `data-path-diagnostics` feature so HIL can activate the
  vendor entry-point instrumentation without changing the normal WPA profiles.
- Added an opt-in `diagnostic-disable-sta-pm` A/B profile. It requires a
  successful typed PM-off result after association before running the unchanged-
  image connectivity matrix; normal example builds retain vendor PM policy.
- Updated `wifi_connectivity` to `hisi-rtos 0.1.0-alpha.15` and the native WS63
  scheduler-port facade. The example no longer owns TIMER_INT0/SOFT_INT0
  handler bodies, `SchedulerPort` wiring, or global-interrupt startup.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.68` and
  `hisi-rtos 0.1.0-alpha.17`. Caller-owned scheduler memory now uses
  `SchedulerArena`, including the HIL-derived synchronization-object headroom
  instead of reserving task stacks alone.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.69`, keeping the example
  on the resource-report v8 diagnostics contract covered by the facade's full
  WPA2 host test.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.70`; its emitted resource
  marker now identifies the repeated-silicon-calibrated WPA2 runtime profile.

### Added

- Added `wifi_connectivity`, the public `hisi-rf 0.1.0-alpha.48` end-to-end
  example covering the incremental runner, scan/connect, smoltcp DHCP, repeated
  ICMP, and lease renewal.
- Added a pinned official nightly and a repository-local Cargo target/linker
  contract so this release unit builds independently of the parent workspace.

### Changed

- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.49` and moved
  profile-specific crypto peripheral ownership into the example's typestate
  resource builder. WPA2 no longer consumes PKE; WPA3 requires it before
  resources can be built.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.52` and replaced the
  public control-storage plus arena pair with one `declare_radio_storage!`
  composition and admission step. Its host-side resource report now uses the
  WS63 RV32 layout rather than the build host's pointer width.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.53`; event capacity is now
  owned by the selected profile and no longer appears in application control
  types or the caller-owned storage declaration.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.54`, moved all
  user-facing radio, operation, application-wait, credential, and runner
  settings into its local `config` module, and adopted distinct typed
  operation/backend timeout contracts. The older compatibility smoke was
  migrated to the same public timeout API.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.55`; the smoltcp runner
  now obtains its station MAC from its own initialized `WifiDevice` instead of
  a process-global netif accessor.
- Made the embedded release profile explicit (`opt-level = "s"`, LTO, debug
  symbols, and one codegen unit), preventing parent-workspace profile drift from
  changing the WS63 SRAM/link layout.
- Migrated every example from the retired `hisi-riscv-hal` package and
  `hisi_riscv_hal` import path to `hisi-hal 0.7.0-alpha.1` / `hisi_hal`.
- **dma_loopback** — retargeted part 2 (mem->mem) from the secure DMA (SDMA
  @0x520A_0000) to the primary M_DMA channel 1. SDMA is never provisioned on WS63
  silicon — a transfer there stalls AXI and hangs the bus — so the example no
  longer exercises it (matches the silicon-faithful `ws63-qemu` DMA model).

### Added
- **xip_flash_clk_hazard** — demonstrates the issue-#4 hazard: re-switching the flash clock (CLDO_CRG_CLK_SEL bit 18) while executing XIP from flash crashes instruction fetch; ws63-qemu now faults it

- **uart_hello** — UART0 serial print example (QEMU-friendly)
- **timer_irq** — TIMER_0 interrupt (IRQ 26) handling example
- **gpio_irq** — GPIO0 pin0 interrupt (IRQ 33) example with custom local IRQ >=32
- **reset_demo** — System reset example (software_reset + reset_reason)
- **dma_loopback** — Peripheral DMA mem<->SPI0 loopback + mem->mem, both on the primary M_DMA
- **wifi_blob_link** — Wi-Fi ROM blob linking spike using hisi-riscv-rt's `.wifi_pkt_ram` symbols
- **rf_port_demo** — ws63-rf-rs porting layer + blob link exercise
- **sched_demo** — ws63-rf-rs cooperative scheduler validation (later moved to ws63-rf-rs)
- **blinky** build.rs — Automatic hisi-riscv-rt linker script discovery (-Tws63-link.x)

### Changed

- **timer_irq, gpio_irq** — Refactored to use hisi_riscv_hal::interrupt controller API
- **wifi_blob_link examples** — Point at nested ws63-RF (ws63-rf-rs/ws63-RF)

### Fixed

- **clippy** — Fixed fn_to_numeric_cast warning in trap-handler (cast through raw pointer)

### Removed

- **sched_demo** — Moved to ws63-rf-rs as an internal example

## [0.1.0]

### Added

- Initial ws63-examples repository with blinky LED example
- **blinky** — GPIO output and busy-wait delay demonstration
  - Uses `hisi-riscv-rt::entry` for startup
  - Uses `hisi-riscv-hal::gpio::create_output_pin` for GPIO control
  - Demonstrates minimal `#![no_std]` + `#![no_main]` embedded application pattern
- Project documentation (ARCHITECTURE.md, README.md)
- Workspace Cargo configuration with path dependencies (ws63-pac, hisi-riscv-hal, hisi-riscv-rt)
