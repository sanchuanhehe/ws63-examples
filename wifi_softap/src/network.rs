//! Bounded IPv4 services for the two-board SoftAP HIL fixture.

use core::num::NonZeroU32;

use hisi_hal::uart::Uart;
use hisi_rf::ws63::{AccessPoint, AccessPointNetworkDevice};
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::udp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    DhcpMessageType, DhcpPacket, DhcpRepr, EthernetAddress, HardwareAddress, IpAddress, IpCidr,
    IpEndpoint, Ipv4Address,
};
#[cfg(feature = "sle-coexistence")]
use ws63_radio_sys::sle::{Address, CONNECTION_STATE_CONNECTED};

const SERVER_ADDRESS: Ipv4Address = Ipv4Address::new(192, 168, 4, 1);
const CLIENT_ADDRESS: Ipv4Address = Ipv4Address::new(192, 168, 4, 2);
const BROADCAST_ADDRESS: Ipv4Address = Ipv4Address::new(255, 255, 255, 255);
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const UDP_ECHO_PORT: u16 = 9;
const DHCP_LEASE_SECONDS: u32 = 20;
#[cfg(feature = "sle-coexistence")]
const SLE_SERVER_ADDRESS: Address = Address {
    address_type: 0,
    bytes: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
};

type Uart0<'d> = Uart<'d, hisi_hal::peripherals::Uart0<'d>>;

#[derive(Clone, Copy)]
struct DhcpRequest {
    message_type: DhcpMessageType,
    transaction_id: u32,
    secs: u16,
    client_hardware_address: EthernetAddress,
    client_ip: Ipv4Address,
    broadcast: bool,
}

#[derive(Default)]
struct NetworkDiagnostics {
    dhcp_discover: u32,
    dhcp_request: u32,
    dhcp_reply: u32,
    dhcp_reply_broadcast: u32,
    dhcp_reply_unicast: u32,
    dhcp_invalid: u32,
    dhcp_last_transaction_id: u32,
    echo_rx: u32,
    echo_tx: u32,
    #[cfg(feature = "data-path-diagnostics")]
    echo_mac_samples: u32,
    #[cfg(feature = "data-path-diagnostics")]
    echo_mac_normal_baseline: u32,
    #[cfg(feature = "data-path-diagnostics")]
    echo_mac_normal_latest: u32,
    #[cfg(feature = "data-path-diagnostics")]
    echo_mac_complete_baseline: u32,
    #[cfg(feature = "data-path-diagnostics")]
    echo_mac_complete_latest: u32,
    #[cfg(feature = "data-path-diagnostics")]
    tx_submission_cursor: u32,
    #[cfg(feature = "data-path-diagnostics")]
    tx_completion_cursor: u32,
}

pub fn run(
    mut access_point: AccessPoint,
    network: AccessPointNetworkDevice,
    #[cfg(feature = "ble-coexistence")] mut ble: hisi_rf_ws63::BleB1Controller,
    #[cfg(feature = "sle-coexistence")] mut sle: hisi_rf_ws63::SleS1Controller,
    uart: &Uart0<'_>,
) -> ! {
    let mut device = network.device;
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(
        network.hardware_address,
    )));
    config.random_seed = 0x5753_4150;
    let mut interface = Interface::new(config, &mut device, now());
    interface.update_ip_addrs(|addresses| {
        addresses
            .push(IpCidr::new(IpAddress::Ipv4(SERVER_ADDRESS), 24))
            .expect("SoftAP IPv4 slot");
    });

    let mut socket_storage = [SocketStorage::EMPTY; 2];
    let mut sockets = SocketSet::new(&mut socket_storage[..]);

    let mut dhcp_rx_metadata = [udp::PacketMetadata::EMPTY; 2];
    let mut dhcp_rx_data = [0_u8; 640];
    let mut dhcp_tx_metadata = [udp::PacketMetadata::EMPTY; 2];
    let mut dhcp_tx_data = [0_u8; 640];
    let dhcp_rx = udp::PacketBuffer::new(&mut dhcp_rx_metadata[..], &mut dhcp_rx_data[..]);
    let dhcp_tx = udp::PacketBuffer::new(&mut dhcp_tx_metadata[..], &mut dhcp_tx_data[..]);
    let mut dhcp = udp::Socket::new(dhcp_rx, dhcp_tx);
    dhcp.bind(DHCP_SERVER_PORT).expect("bind DHCP server");
    let dhcp_handle = sockets.add(dhcp);

    // A single interface poll can enqueue the complete bounded echo burst
    // before service_echo runs. Keep enough packet slots so smoltcp does not
    // silently discard later datagrams.
    let mut echo_rx_metadata = [udp::PacketMetadata::EMPTY; 16];
    let mut echo_rx_data = [0_u8; 512];
    let mut echo_tx_metadata = [udp::PacketMetadata::EMPTY; 2];
    let mut echo_tx_data = [0_u8; 512];
    let echo_rx = udp::PacketBuffer::new(&mut echo_rx_metadata[..], &mut echo_rx_data[..]);
    let echo_tx = udp::PacketBuffer::new(&mut echo_tx_metadata[..], &mut echo_tx_data[..]);
    let mut echo = udp::Socket::new(echo_rx, echo_tx);
    echo.bind(UDP_ECHO_PORT).expect("bind local UDP echo");
    let echo_handle = sockets.add(echo);

    uart.write(b"RFDBG_SOFTAP_NET_READY ip=192.168.4.1 lease=192.168.4.2 echo=9\r\n");
    let mut diagnostics = NetworkDiagnostics::default();
    #[cfg(feature = "data-path-diagnostics")]
    {
        let timeline = access_point.diagnostics().data_tx_timeline;
        diagnostics.tx_submission_cursor = timeline.submission_total;
        diagnostics.tx_completion_cursor = timeline.completion_total;
    }
    let mut next_diagnostic_ms = crate::monotonic_ms();
    let mut next_task_diagnostic_ms = crate::monotonic_ms();
    #[cfg(feature = "sle-coexistence")]
    let mut sle_started = false;
    #[cfg(feature = "ble-coexistence")]
    let mut ble_started = false;
    loop {
        #[cfg(feature = "ble-coexistence")]
        service_ble(&mut ble, &mut ble_started, uart);
        #[cfg(feature = "sle-coexistence")]
        service_sle(&mut sle, &mut sle_started, uart);
        access_point
            .poll(NonZeroU32::new(16).unwrap())
            .expect("poll native authenticator");
        let timestamp = now();
        let _ = interface.poll(timestamp, &mut device, &mut sockets);
        service_dhcp(&mut sockets, dhcp_handle, &mut diagnostics);
        let echo_sequence = service_echo(&mut sockets, echo_handle, &mut diagnostics);
        #[cfg(not(feature = "data-path-diagnostics"))]
        let _ = echo_sequence;
        #[cfg(feature = "data-path-diagnostics")]
        let echo_before = echo_sequence.map(|_| access_point.diagnostics());
        #[cfg(feature = "data-path-diagnostics")]
        if let Some(before) = echo_before
            && diagnostics.echo_mac_samples == 0
        {
            diagnostics.echo_mac_normal_baseline = before.mac_tx_normal_priority_mpdu;
            diagnostics.echo_mac_complete_baseline = before.mac_tx_complete_interrupts;
        }
        let _ = interface.poll(timestamp, &mut device, &mut sockets);
        access_point.wait_for_work(5).expect("wait for AP work");
        #[cfg(feature = "data-path-diagnostics")]
        if let (Some(sequence), Some(before)) = (echo_sequence, echo_before) {
            let after = access_point.diagnostics();
            diagnostics.echo_mac_samples = diagnostics.echo_mac_samples.saturating_add(1);
            diagnostics.echo_mac_normal_latest = after.mac_tx_normal_priority_mpdu;
            diagnostics.echo_mac_complete_latest = after.mac_tx_complete_interrupts;
            write_echo_path_diagnostics(uart, sequence, before, after);
        }
        #[cfg(feature = "data-path-diagnostics")]
        write_new_tx_timeline(uart, access_point.diagnostics(), &mut diagnostics);

        let current_ms = crate::monotonic_ms();
        if current_ms.wrapping_sub(next_diagnostic_ms) >= 1_000 {
            #[cfg(feature = "data-path-diagnostics")]
            if diagnostics.echo_mac_samples != 0 {
                let latest = access_point.diagnostics();
                diagnostics.echo_mac_normal_latest = latest.mac_tx_normal_priority_mpdu;
                diagnostics.echo_mac_complete_latest = latest.mac_tx_complete_interrupts;
            }
            crate::write_diagnostics(uart, access_point.diagnostics());
            #[cfg(feature = "wpa3")]
            write_wpa3_crypto_diagnostics(uart);
            write_network_diagnostics(uart, &diagnostics);
            next_diagnostic_ms = current_ms;
        }
        if current_ms.wrapping_sub(next_task_diagnostic_ms) >= 5_000 {
            crate::write_rtos_task_diagnostics(uart);
            next_task_diagnostic_ms = current_ms;
        }
    }
}

#[cfg(feature = "ble-coexistence")]
fn service_ble(
    controller: &mut hisi_rf_ws63::BleB1Controller,
    started: &mut bool,
    uart: &Uart0<'_>,
) {
    static ADVERTISING_DATA: &[u8] = &[
        2, 0x01, 0x06, 9, 0x09, b'H', b'I', b'S', b'I', b'C', b'O', b'E', b'X',
    ];

    while let Some(event) = controller.next_event() {
        match event {
            hisi_rf_ws63::BleB2Event::Enabled { status: 0 } if !*started => {
                controller
                    .start_advertising(ADVERTISING_DATA)
                    .expect("start BLE coexistence advertising");
                *started = true;
            }
            hisi_rf_ws63::BleB2Event::AdvertisingState { status: 1, .. } => {
                uart.write(b"RFDBG_COEX_BLE_SERVER_READY\r\n");
            }
            hisi_rf_ws63::BleB2Event::ConnectionState {
                connected: true, ..
            } => {
                uart.write(b"RFDBG_COEX_BLE_SERVER_CONNECTED\r\n");
            }
            hisi_rf_ws63::BleB2Event::Enabled { status }
            | hisi_rf_ws63::BleB2Event::AdvertisingData { status, .. }
            | hisi_rf_ws63::BleB2Event::AdvertisingParameters { status, .. }
            | hisi_rf_ws63::BleB2Event::AdvertisingState { status, .. } => {
                uart.write(b"RFDBG_COEX_BLE_SERVER_ERR status=0x");
                uart.write(&crate::hex8(status));
                uart.write(b"\r\n");
                panic!("BLE server event failure");
            }
            _ => {}
        }
    }
    if controller.dropped_events() != 0 {
        uart.write(b"RFDBG_COEX_BLE_SERVER_EVENT_DROP\r\n");
        panic!("BLE server event queue overflow");
    }
}

#[cfg(feature = "sle-coexistence")]
fn service_sle(
    controller: &mut hisi_rf_ws63::SleS1Controller,
    started: &mut bool,
    uart: &Uart0<'_>,
) {
    while let Some(event) = controller.next_event() {
        match event {
            hisi_rf_ws63::SleS1Event::Enabled { status: 0 } if !*started => {
                static mut ANNOUNCE_DATA: [u8; 7] = [1, 1, 1, 3, 2, 0x0b, 0x06];
                static mut SEEK_RESPONSE_DATA: [u8; 10] =
                    [5, 8, b'H', b'I', b'S', b'I', b'S', b'L', b'E', b'2'];
                if controller.set_local_address(SLE_SERVER_ADDRESS).is_err() {
                    panic!("set SLE server address");
                }
                let announce = unsafe { &mut *core::ptr::addr_of_mut!(ANNOUNCE_DATA) };
                let response = unsafe { &mut *core::ptr::addr_of_mut!(SEEK_RESPONSE_DATA) };
                if controller.start_announce(announce, response).is_err() {
                    panic!("start SLE server announce");
                }
                *started = true;
            }
            hisi_rf_ws63::SleS1Event::AnnounceEnabled { status: 0, .. } => {
                uart.write(b"RFDBG_COEX_SLE_SERVER_READY\r\n");
            }
            hisi_rf_ws63::SleS1Event::ConnectionStateChanged {
                connection_state: CONNECTION_STATE_CONNECTED,
                ..
            } => {
                uart.write(b"RFDBG_COEX_SLE_SERVER_CONNECTED\r\n");
            }
            hisi_rf_ws63::SleS1Event::Enabled { status }
            | hisi_rf_ws63::SleS1Event::AnnounceEnabled { status, .. } => {
                uart.write(b"RFDBG_COEX_SLE_SERVER_ERR status=0x");
                uart.write(&crate::hex8(status));
                uart.write(b"\r\n");
                panic!("SLE server event failure");
            }
            _ => {}
        }
    }
    if controller.dropped_events() != 0 {
        uart.write(b"RFDBG_COEX_SLE_SERVER_EVENT_DROP\r\n");
        panic!("SLE server event queue overflow");
    }
}

#[cfg(feature = "wpa3")]
fn write_wpa3_crypto_diagnostics(uart: &Uart0<'_>) {
    let point = hisi_rf::ws63::hardware_p256_diagnostic_snapshot();
    let field = hisi_rf::ws63::hardware_p256_field_diagnostic_snapshot();
    let curve = hisi_rf::ws63::hardware_p256_curve_diagnostic_snapshot();
    uart.write(b"RFDBG_SOFTAP_SAE p256_req=");
    uart.write(&crate::hex8(point[0]));
    uart.write(b" p256_fail=");
    uart.write(&crate::hex8(point[1]));
    uart.write(b" field_req=");
    uart.write(&crate::hex8(field[0]));
    uart.write(b" field_fail=");
    uart.write(&crate::hex8(field[1]));
    uart.write(b" curve_req=");
    uart.write(&crate::hex8(curve[0]));
    uart.write(b" curve_fail=");
    uart.write(&crate::hex8(curve[1]));
    uart.write(b"\r\n");
}

fn service_dhcp(
    sockets: &mut SocketSet<'_>,
    handle: smoltcp::iface::SocketHandle,
    diagnostics: &mut NetworkDiagnostics,
) {
    let request = {
        let socket = sockets.get_mut::<udp::Socket>(handle);
        let Ok((payload, _metadata)) = socket.recv() else {
            return;
        };
        parse_dhcp_request(payload)
    };
    let Some(request) = request else {
        diagnostics.dhcp_invalid = diagnostics.dhcp_invalid.saturating_add(1);
        return;
    };
    let reply_type = match request.message_type {
        DhcpMessageType::Discover => {
            diagnostics.dhcp_discover = diagnostics.dhcp_discover.saturating_add(1);
            DhcpMessageType::Offer
        }
        DhcpMessageType::Request => {
            diagnostics.dhcp_request = diagnostics.dhcp_request.saturating_add(1);
            DhcpMessageType::Ack
        }
        _ => return,
    };
    let mut payload = [0_u8; 300];
    let Some(length) = emit_dhcp_reply(request, reply_type, &mut payload) else {
        diagnostics.dhcp_invalid = diagnostics.dhcp_invalid.saturating_add(1);
        return;
    };
    let reply_address = if request.broadcast || request.client_ip == Ipv4Address::UNSPECIFIED {
        BROADCAST_ADDRESS
    } else {
        request.client_ip
    };
    let endpoint = IpEndpoint::new(IpAddress::Ipv4(reply_address), DHCP_CLIENT_PORT);
    if sockets
        .get_mut::<udp::Socket>(handle)
        .send_slice(&payload[..length], endpoint)
        .is_ok()
    {
        diagnostics.dhcp_reply = diagnostics.dhcp_reply.saturating_add(1);
        if reply_address == BROADCAST_ADDRESS {
            diagnostics.dhcp_reply_broadcast = diagnostics.dhcp_reply_broadcast.saturating_add(1);
        } else {
            diagnostics.dhcp_reply_unicast = diagnostics.dhcp_reply_unicast.saturating_add(1);
        }
        diagnostics.dhcp_last_transaction_id = request.transaction_id;
    }
}

fn parse_dhcp_request(payload: &[u8]) -> Option<DhcpRequest> {
    let packet = DhcpPacket::new_checked(payload).ok()?;
    let repr = DhcpRepr::parse(&packet).ok()?;
    Some(DhcpRequest {
        message_type: repr.message_type,
        transaction_id: repr.transaction_id,
        secs: repr.secs,
        client_hardware_address: repr.client_hardware_address,
        client_ip: repr.client_ip,
        broadcast: repr.broadcast,
    })
}

fn emit_dhcp_reply(
    request: DhcpRequest,
    message_type: DhcpMessageType,
    output: &mut [u8],
) -> Option<usize> {
    let repr = DhcpRepr {
        message_type,
        transaction_id: request.transaction_id,
        secs: request.secs,
        client_hardware_address: request.client_hardware_address,
        client_ip: Ipv4Address::UNSPECIFIED,
        your_ip: CLIENT_ADDRESS,
        server_ip: SERVER_ADDRESS,
        // This fixture is intentionally isolated. Advertising a default
        // router would make the STA treat public reachability as an AP
        // contract even though this firmware performs no forwarding.
        router: None,
        subnet_mask: Some(Ipv4Address::new(255, 255, 255, 0)),
        relay_agent_ip: Ipv4Address::UNSPECIFIED,
        broadcast: request.broadcast || request.client_ip == Ipv4Address::UNSPECIFIED,
        requested_ip: None,
        client_identifier: None,
        server_identifier: Some(SERVER_ADDRESS),
        parameter_request_list: None,
        dns_servers: None,
        max_size: None,
        lease_duration: Some(DHCP_LEASE_SECONDS),
        renew_duration: Some(DHCP_LEASE_SECONDS / 2),
        rebind_duration: Some(DHCP_LEASE_SECONDS * 7 / 8),
        additional_options: &[],
    };
    let length = repr.buffer_len();
    if length > output.len() {
        return None;
    }
    repr.emit(&mut DhcpPacket::new_unchecked(&mut output[..length]))
        .ok()?;
    Some(length)
}

fn service_echo(
    sockets: &mut SocketSet<'_>,
    handle: smoltcp::iface::SocketHandle,
    diagnostics: &mut NetworkDiagnostics,
) -> Option<u8> {
    let mut payload = [0_u8; 256];
    let received = sockets
        .get_mut::<udp::Socket>(handle)
        .recv_slice(&mut payload);
    let Ok((length, metadata)) = received else {
        return None;
    };
    diagnostics.echo_rx = diagnostics.echo_rx.saturating_add(1);
    if sockets
        .get_mut::<udp::Socket>(handle)
        .send_slice(&payload[..length], metadata)
        .is_ok()
    {
        diagnostics.echo_tx = diagnostics.echo_tx.saturating_add(1);
        (length == 1).then_some(payload[0])
    } else {
        None
    }
}

#[cfg(feature = "data-path-diagnostics")]
fn write_echo_path_diagnostics(
    uart: &Uart0<'_>,
    sequence: u8,
    before: hisi_rf::ws63::AccessPointDiagnostics,
    after: hisi_rf::ws63::AccessPointDiagnostics,
) {
    uart.write(b"RFDBG_SOFTAP_ECHO_PATH sequence=0x");
    uart.write(&crate::hex8(u32::from(sequence)));
    uart.write(b" vendor_tx_delta=0x");
    uart.write(&crate::hex8(
        after
            .data_vendor_tx_frames
            .wrapping_sub(before.data_vendor_tx_frames),
    ));
    uart.write(b" tx_complete_delta=0x");
    uart.write(&crate::hex8(
        after
            .data_tx_completions
            .wrapping_sub(before.data_tx_completions),
    ));
    uart.write(b" mac_norm_delta=0x");
    uart.write(&crate::hex8(
        after
            .mac_tx_normal_priority_mpdu
            .wrapping_sub(before.mac_tx_normal_priority_mpdu),
    ));
    uart.write(b" mac_irq_delta=0x");
    uart.write(&crate::hex8(
        after
            .mac_tx_complete_interrupts
            .wrapping_sub(before.mac_tx_complete_interrupts),
    ));
    uart.write(b" tx_status_delta=");
    for (before, after) in before
        .data_tx_completion_status
        .into_iter()
        .zip(after.data_tx_completion_status)
    {
        uart.write(&crate::hex8(after.wrapping_sub(before)));
        uart.write(b",");
    }
    uart.write(b"\r\n");
}

#[cfg(feature = "data-path-diagnostics")]
fn write_new_tx_timeline(
    uart: &Uart0<'_>,
    snapshot: hisi_rf::ws63::AccessPointDiagnostics,
    diagnostics: &mut NetworkDiagnostics,
) {
    let timeline = snapshot.data_tx_timeline;
    let slots = timeline.submissions.len() as u32;
    diagnostics.tx_submission_cursor = diagnostics
        .tx_submission_cursor
        .max(timeline.submission_total.saturating_sub(slots));
    while diagnostics.tx_submission_cursor < timeline.submission_total {
        let index = diagnostics.tx_submission_cursor;
        let slot = index as usize % timeline.submissions.len();
        uart.write(b"RFDBG_SOFTAP_TX_SUBMIT index=0x");
        uart.write(&crate::hex8(index));
        uart.write(b" time_ms=0x");
        uart.write(&crate::hex8(timeline.submission_time_ms[slot]));
        uart.write(b" echo=0x");
        uart.write(&crate::hex8(timeline.submissions[slot]));
        uart.write(b"\r\n");
        diagnostics.tx_submission_cursor = index.wrapping_add(1);
    }

    diagnostics.tx_completion_cursor = diagnostics
        .tx_completion_cursor
        .max(timeline.completion_total.saturating_sub(slots));
    while diagnostics.tx_completion_cursor < timeline.completion_total {
        let index = diagnostics.tx_completion_cursor;
        let slot = index as usize % timeline.completions.len();
        uart.write(b"RFDBG_SOFTAP_TX_COMPLETE index=0x");
        uart.write(&crate::hex8(index));
        uart.write(b" time_ms=0x");
        uart.write(&crate::hex8(timeline.completion_time_ms[slot]));
        uart.write(b" descriptor=0x");
        uart.write(&crate::hex8(timeline.completions[slot]));
        uart.write(b" packet_number=0x");
        uart.write(&crate::hex8(timeline.packet_number_lsb[slot]));
        uart.write(b" echo=0x");
        uart.write(&crate::hex8(timeline.completion_echo[slot]));
        uart.write(b"\r\n");
        diagnostics.tx_completion_cursor = index.wrapping_add(1);
    }
}

fn write_network_diagnostics(uart: &Uart0<'_>, diagnostics: &NetworkDiagnostics) {
    uart.write(b"RFDBG_SOFTAP_NET dhcp_discover=");
    uart.write(&crate::hex8(diagnostics.dhcp_discover));
    uart.write(b" dhcp_request=");
    uart.write(&crate::hex8(diagnostics.dhcp_request));
    uart.write(b" dhcp_reply=");
    uart.write(&crate::hex8(diagnostics.dhcp_reply));
    uart.write(b" dhcp_reply_broadcast=");
    uart.write(&crate::hex8(diagnostics.dhcp_reply_broadcast));
    uart.write(b" dhcp_reply_unicast=");
    uart.write(&crate::hex8(diagnostics.dhcp_reply_unicast));
    uart.write(b" dhcp_invalid=");
    uart.write(&crate::hex8(diagnostics.dhcp_invalid));
    uart.write(b" dhcp_last_xid=");
    uart.write(&crate::hex8(diagnostics.dhcp_last_transaction_id));
    uart.write(b" echo_rx=");
    uart.write(&crate::hex8(diagnostics.echo_rx));
    uart.write(b" echo_tx=");
    uart.write(&crate::hex8(diagnostics.echo_tx));
    #[cfg(feature = "data-path-diagnostics")]
    {
        uart.write(b" echo_mac_samples=");
        uart.write(&crate::hex8(diagnostics.echo_mac_samples));
        uart.write(b" echo_mac_norm_delta=");
        uart.write(&crate::hex8(
            diagnostics
                .echo_mac_normal_latest
                .wrapping_sub(diagnostics.echo_mac_normal_baseline),
        ));
        uart.write(b" echo_mac_irq_delta=");
        uart.write(&crate::hex8(
            diagnostics
                .echo_mac_complete_latest
                .wrapping_sub(diagnostics.echo_mac_complete_baseline),
        ));
    }
    uart.write(b"\r\n");

    let l2 = hisi_rf::ws63::netif_smoltcp::l2_protocol_diagnostics();
    uart.write(b"RFDBG_SOFTAP_L2 tx_count=");
    uart.write(&crate::hex8(hisi_rf::ws63::netif_smoltcp::tx_count()));
    uart.write(b" tx_failed=");
    uart.write(&crate::hex8(hisi_rf::ws63::netif::tx_failed()));
    uart.write(b" rx_arp_req=");
    uart.write(&crate::hex8(l2.rx_arp_requests));
    uart.write(b" rx_ipv4=");
    uart.write(&crate::hex8(l2.rx_ipv4));
    uart.write(b" tx_arp_reply=");
    uart.write(&crate::hex8(l2.tx_arp_replies));
    uart.write(b" tx_ipv4=");
    uart.write(&crate::hex8(l2.tx_ipv4));

    let rx_queue = hisi_rf::ws63::netif_smoltcp::rx_queue_diagnostics();
    uart.write(b" rx_pending=");
    uart.write(&crate::hex8(rx_queue.pending as u32));
    uart.write(b" rx_high_water=");
    uart.write(&crate::hex8(rx_queue.high_watermark as u32));
    uart.write(b" rx_dropped=");
    uart.write(&crate::hex8(rx_queue.dropped));

    let mut frame = [0_u8; 64];
    let length = hisi_rf::ws63::netif_smoltcp::last_tx(&mut frame);
    uart.write(b" last_len=");
    uart.write(&crate::hex8(length as u32));
    if length >= 14 {
        uart.write(b" last_dst=");
        write_hex_bytes(uart, &frame[..6]);
        uart.write(b" last_src=");
        write_hex_bytes(uart, &frame[6..12]);
        uart.write(b" ethertype=");
        write_hex_bytes(uart, &frame[12..14]);
    }
    if length >= 42 && frame[12..14] == [0x08, 0x00] && frame[23] == 17 {
        let udp = 14 + usize::from(frame[14] & 0x0f) * 4;
        if udp + 8 <= length.min(frame.len()) {
            uart.write(b" ip_dst=");
            write_ipv4(uart, [frame[30], frame[31], frame[32], frame[33]]);
            uart.write(b" ip_len=");
            uart.write(&crate::hex8(u32::from(u16::from_be_bytes([
                frame[16], frame[17],
            ]))));
            uart.write(b" ip_sum=");
            uart.write(&crate::hex8(u32::from(u16::from_be_bytes([
                frame[24], frame[25],
            ]))));
            uart.write(b" udp_src=");
            uart.write(&crate::hex8(u32::from(u16::from_be_bytes([
                frame[udp],
                frame[udp + 1],
            ]))));
            uart.write(b" udp_dst=");
            uart.write(&crate::hex8(u32::from(u16::from_be_bytes([
                frame[udp + 2],
                frame[udp + 3],
            ]))));
            uart.write(b" udp_len=");
            uart.write(&crate::hex8(u32::from(u16::from_be_bytes([
                frame[udp + 4],
                frame[udp + 5],
            ]))));
            uart.write(b" udp_sum=");
            uart.write(&crate::hex8(u32::from(u16::from_be_bytes([
                frame[udp + 6],
                frame[udp + 7],
            ]))));
        }
    }
    uart.write(b"\r\n");
}

fn write_hex_bytes(uart: &Uart0<'_>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        uart.write(&[HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]]);
    }
}

fn write_ipv4(uart: &Uart0<'_>, address: [u8; 4]) {
    for (index, octet) in address.into_iter().enumerate() {
        if index != 0 {
            uart.write(b".");
        }
        let mut digits = [0_u8; 3];
        let mut value = octet;
        let mut cursor = digits.len();
        loop {
            cursor -= 1;
            digits[cursor] = b'0' + value % 10;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        uart.write(&digits[cursor..]);
    }
}

fn now() -> Instant {
    Instant::from_millis(crate::monotonic_ms() as i64)
}
