use crate::dither::DitherApply;
use crate::hex::decode_hex;
use crate::image::generate_image;
use crate::instruction::{
    DISABLE_SHUTDOWN, PRINTER_NAME_PREFIX, SET_THICKNESS, STOP_PRINT_JOBS, WRITE_UUID,
};
use actix_web::rt::time;
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use std::io::ErrorKind;
use std::time::Duration;
use uuid::Uuid;

async fn get_central(manager: &Manager) -> Adapter {
    let adapters = manager.adapters().await.unwrap();
    adapters.into_iter().next().unwrap()
}

pub async fn init_printer() -> anyhow::Result<(Peripheral, Characteristic)> {
    let manager = Manager::new().await?;

    // get the first bluetooth adapter
    // connect to the adapter
    let central = get_central(&manager).await;

    // start scanning for devices
    central
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| std::io::Error::new(ErrorKind::Interrupted, e))?;
    // instead of waiting, you can use central.events() to get a stream which will
    // notify you of new devices, for an example of that see examples/event_driven_discovery.rs
    time::sleep(Duration::from_secs(2)).await;

    let peripherals = central
        .peripherals()
        .await
        .map_err(|e| std::io::Error::new(ErrorKind::Interrupted, e))?;
    if peripherals.is_empty() {
        panic!("->>> BLE peripheral devices were not found, sorry. Exiting...");
    }

    let printer = find_printer(peripherals)
        .await
        .expect("printer not start or it is linking other device");

    log::debug!("{:?}", printer);

    // connect to the device
    printer.connect().await?;

    // discover services and characteristics
    printer.discover_services().await?;

    // find the characteristic we want
    let chars = printer.characteristics();

    let find_char = |uuid: Uuid| {
        chars
            .iter()
            .find(|c| c.uuid == uuid)
            .expect("unable to find characteristics")
    };
    let cmd_char = find_char(WRITE_UUID);

    printer
        .write(
            cmd_char,
            DISABLE_SHUTDOWN.as_slice(),
            WriteType::WithResponse,
        )
        .await?;

    printer
        .write(cmd_char, SET_THICKNESS.as_slice(), WriteType::WithResponse)
        .await?;

    Ok((printer, cmd_char.clone()))
}

#[allow(clippy::await_holding_lock)]
pub async fn call_printer(
    text: &str,
    printer: &Peripheral,
    cmd_char: &Characteristic,
) -> anyhow::Result<()> {
    let buffer = generate_image(None, Some(text)).unwrap();

    let mut dither_apply = DitherApply::new(buffer);
    let image_hex_str = dither_apply.make_image_hex_str();

    let hex_len = format!("{:X}", (image_hex_str.len() / 96) + 3);
    let mut front_hex = hex_len.clone();
    let mut end_hex = String::from("0");

    if hex_len.len() > 2 {
        front_hex = hex_len[1..].to_string();
        end_hex += hex_len[0..1].to_string().as_str();
    } else {
        end_hex += "0";
    }

    let mut data = format!(
        "{:0<32}",
        String::from("1D7630003000") + &*front_hex + &*end_hex
    );
    data += &image_hex_str[0..224];

    printer
        .write(
            cmd_char,
            decode_hex(data.as_str()).unwrap().as_slice(),
            WriteType::WithResponse,
        )
        .await?;

    // send image data in chunks
    for i in (224..image_hex_str.len()).step_by(256) {
        let str = &*format!("{:0<256}", unsafe {
            image_hex_str.get_unchecked(i..i + 256)
        });
        printer
            .write(
                cmd_char,
                decode_hex(str).unwrap().as_slice(),
                WriteType::WithResponse,
            )
            .await?;
    }

    printer
        .write(
            cmd_char,
            STOP_PRINT_JOBS.as_slice(),
            WriteType::WithResponse,
        )
        .await?;

    Ok(())
}

async fn find_printer(peripherals: Vec<Peripheral>) -> Option<Peripheral> {
    for p in peripherals {
        if p.properties()
            .await
            .unwrap()
            .unwrap()
            .local_name
            .iter()
            .any(|name| name.contains(PRINTER_NAME_PREFIX))
        {
            return Some(p);
        }
    }
    None
}