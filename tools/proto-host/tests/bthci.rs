use proto_host::bthci::*;

#[test]
fn commands_have_the_spec_bytes() {
    assert_eq!(reset().as_slice(), [0x01, 0x03, 0x0c, 0x00]);
    assert_eq!(read_local_version().as_slice(), [0x01, 0x01, 0x10, 0x00]);
    assert_eq!(read_bd_addr().as_slice(), [0x01, 0x09, 0x10, 0x00]);
    assert_eq!(
        inquiry(8, 0).as_slice(),
        [0x01, 0x01, 0x04, 0x05, 0x33, 0x8b, 0x9e, 8, 0]
    );
    assert_eq!(
        le_set_scan_parameters(0x0010, 0x0010).as_slice(),
        [0x01, 0x0b, 0x20, 0x07, 0, 0x10, 0, 0x10, 0, 0, 0]
    );
    assert_eq!(le_set_scan_enable(true).as_slice(), [0x01, 0x0c, 0x20, 0x02, 1, 1]);
    assert_eq!(
        set_event_mask(0x2000_1fff_ffff_ffff).as_slice(),
        [0x01, 0x01, 0x0c, 0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x00, 0x20]
    );
    assert!(command(RESET, &[0; 256]).is_none());
}

#[test]
fn events_are_decoded() {
    let complete = [0x04, 0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
    assert_eq!(
        parse_event(&complete),
        Some(Event::CommandComplete { opcode: RESET, status: 0, parameters: &[] })
    );
    let status = [0x04, 0x0f, 0x04, 0x0c, 0x01, 0x01, 0x04];
    assert_eq!(
        parse_event(&status),
        Some(Event::CommandStatus { opcode: INQUIRY, status: 0x0c })
    );
    assert_eq!(parse_event(&[0x04, 0x05, 0x04, 0, 0, 0, 0]), Some(Event::Other(0x05)));
    assert_eq!(parse_event(&[0x01, 0, 0]), None);
    assert_eq!(parse_event(&[0x04, 0x0e, 0x09, 1]), None);
}

#[test]
fn advertising_reports_carry_address_data_and_signal() {
    // Meta event 0x3e, sub-event 2, one report: kind 0, public address,
    // data "AB" and an RSSI of -56 dBm.
    let body = [0x02, 0x01, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 0x02, 0x41, 0x42, 0xc8];
    let mut packet = vec![0x04, 0x3e, body.len() as u8];
    packet.extend_from_slice(&body);
    match parse_event(&packet) {
        Some(Event::LeAdvertisement { address, address_type, kind, rssi, data }) => {
            assert_eq!(address, [1, 2, 3, 4, 5, 6]);
            assert_eq!((address_type, kind, rssi), (0, 0, -56));
            assert_eq!(data, b"AB");
        }
        other => panic!("not an advertisement: {other:?}"),
    }
}

#[test]
fn acl_header_is_split() {
    let packet = [0x02, 0x2a, 0x20, 0x03, 0x00, 1, 2, 3];
    assert_eq!(parse_acl_header(&packet), Some((0x02a, 2, 3)));
    assert_eq!(parse_acl_header(&packet[..7]), None);
}

fn complete(opcode: u16, parameters: &[u8], status: u8) -> Vec<u8> {
    let mut packet = vec![0x04, 0x0e, (4 + parameters.len()) as u8, 1];
    packet.extend_from_slice(&opcode.to_le_bytes());
    packet.push(status);
    packet.extend_from_slice(parameters);
    packet
}

#[test]
fn bringup_walks_the_controller_through_its_queries() {
    let mut host = Bringup::new();
    assert_eq!(host.command().unwrap().as_slice(), reset().as_slice());
    host.event(&complete(RESET, &[], 0));
    assert_eq!(host.command().unwrap().as_slice(), read_local_version().as_slice());
    // A completion for another command does not move on.
    host.event(&complete(READ_BD_ADDR, &[1, 2, 3, 4, 5, 6], 0));
    assert_eq!(host.stage, Stage::Version);
    host.event(&complete(READ_LOCAL_VERSION, &[9, 0x34, 0x12, 9, 0x0f, 0x00, 0x56, 0x78], 0));
    assert_eq!(host.command().unwrap().as_slice(), read_bd_addr().as_slice());
    host.event(&complete(READ_BD_ADDR, &[0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6], 0));
    assert_eq!(host.stage, Stage::Ready);
    assert!(host.command().is_none());
    assert_eq!(host.controller.hci_version, 9);
    assert_eq!(host.controller.manufacturer, 0x000f);
    assert_eq!(host.controller.address, [0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6]);
}

#[test]
fn a_refused_command_stops_the_bringup() {
    let mut host = Bringup::new();
    host.event(&complete(RESET, &[], 0x0c));
    assert_eq!(host.stage, Stage::Failed(0x0c));
    assert!(host.command().is_none());
}
