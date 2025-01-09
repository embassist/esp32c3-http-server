// ANCHOR: client_deps
#![no_std]
#![no_main]

use core::fmt::{Debug, Display};
use embedded_io_async::{Read, Write};
use log::info;
use esp_hal::{
    prelude::*,
    rng::Rng,
    time::{self, Duration},
};

extern crate alloc;
use esp_alloc as _;
use esp_backtrace as _;
use esp_println::{print, println};

use blocking_network_stack::Stack;
use embedded_io::*;
use esp_wifi::{
    init,
    wifi::{
        utils::create_network_interface, AccessPointInfo, AuthMethod, ClientConfiguration,
        Configuration, WifiError, WifiStaDevice,
    },
};

use smoltcp::{
    iface::{SocketSet, SocketStorage},
    wire::{DhcpOption, IpAddress, Ipv4Address},
};
// ANCHOR_END: client_deps

// ANCHOR: server_deps
// use core::fmt::{Debug, Display};

use edge_http::io::server::{Connection, DefaultServer, Handler};
use edge_http::io::Error;
use edge_http::Method;
use edge_nal::TcpBind;

// use embedded_io_async::{Read, Write};

// use log::info;
// ANCHOR_END: server_deps

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[entry]
fn main() -> ! {
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config.cpu_clock = CpuClock::max();
        config
    });

    esp_alloc::heap_allocator!(72 * 1024);

    // Initialize the timers used for Wifi
    // ANCHOR: wifi_init
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let mut rng = Rng::new(peripherals.RNG);
    let init = init(timg0.timer0, rng.clone(), peripherals.RADIO_CLK).unwrap();
    // ANCHOR_END: wifi_init

    // Configure Wifi
    // ANCHOR: wifi_config
    let mut wifi = peripherals.WIFI;
    let (iface, device, mut controller) =
        create_network_interface(&init, &mut wifi, WifiStaDevice).unwrap();
    // ANCHOR_END: wifi_config

    let mut auth_method = AuthMethod::WPA2Personal;
    if PASSWORD.is_empty() {
        auth_method = AuthMethod::None;
    }

    // ANCHOR: client_config_start
    let client_config = Configuration::Client(ClientConfiguration {
        // ANCHOR_END: client_config_start
        ssid: SSID.try_into().unwrap(),
        password: PASSWORD.try_into().unwrap(),
        auth_method,
        ..Default::default() // ANCHOR: client_config_end
    });

    let res = controller.set_configuration(&client_config);
    println!("Wi-Fi set_configuration returned {:?}", res);
    // ANCHOR_END: client_config_end

    // ANCHOR: wifi_connect
    controller.start().unwrap();
    println!("Is wifi started: {:?}", controller.is_started());

    println!("Start Wifi Scan");
    let res: Result<(heapless::Vec<AccessPointInfo, 10>, usize), WifiError> = controller.scan_n();
    if let Ok((res, _count)) = res {
        for ap in res {
            println!("{:?}", ap);
        }
    }

    println!("{:?}", controller.capabilities());
    println!("Wi-Fi connect: {:?}", controller.connect());

    // Wait to get connected
    println!("Wait to get connected");
    loop {
        let res = controller.is_connected();
        match res {
            Ok(connected) => {
                if connected {
                    break;
                }
            }
            Err(err) => {
                println!("{:?}", err);
                loop {}
            }
        }
    }
    println!("{:?}", controller.is_connected());
    // ANCHOR_END: wifi_connect

    // ANCHOR: ip
    let mut socket_set_entries: [SocketStorage; 3] = Default::default();
    let mut socket_set = SocketSet::new(&mut socket_set_entries[..]);
    let mut dhcp_socket = smoltcp::socket::dhcpv4::Socket::new();
    // we can set a hostname here (or add other DHCP options)
    dhcp_socket.set_outgoing_options(&[DhcpOption {
        kind: 12,
        data: b"esp-wifi",
    }]);
    socket_set.add(dhcp_socket);
    // Wait for getting an ip address
    let now = || time::now().duration_since_epoch().to_millis();
    let wifi_stack = Stack::new(iface, device, socket_set, now, rng.random());
    println!("Wait to get an ip address");
    loop {
        wifi_stack.work();

        if wifi_stack.is_iface_up() {
            println!("got ip {:?}", wifi_stack.get_ip_info());
            break;
        }
    }
    // ANCHOR_END: ip

    println!("Start busy loop on main");

    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];
    let mut socket = wifi_stack.get_socket(&mut rx_buffer, &mut tx_buffer);

    loop {
        println!("Making HTTP request");
        socket.work();

        socket
            .open(IpAddress::Ipv4(Ipv4Address::new(142, 250, 185, 115)), 80)
            .unwrap();

        socket
            .write(b"GET / HTTP/1.0\r\nHost: www.mobile-j.de\r\n\r\n")
            .unwrap();
        socket.flush().unwrap();

        // ANCHOR: reponse
        let deadline = time::now() + Duration::secs(20);
        let mut buffer = [0u8; 512];
        while let Ok(len) = socket.read(&mut buffer) {
            let to_print = unsafe { core::str::from_utf8_unchecked(&buffer[..len]) };
            print!("{}", to_print);

            if time::now() > deadline {
                println!("Timeout");
                break;
            }
        }
        println!();
        // ANCHOR_END: reponse

        // ANCHOR: socket_close
        socket.disconnect();

        let deadline = time::now() + Duration::secs(5);
        while time::now() < deadline {
            socket.work();
        }
        // ANCHOR_END: socket_close
    }
}

pub async fn run(server: &mut DefaultServer) -> Result<(), anyhow::Error> {
    let addr = "0.0.0.0:8881";

    info!("Running HTTP server on {addr}");

    let acceptor = edge_nal_std::Stack::new()
        .bind(addr.parse().unwrap())
        .await?;

    server.run(None, acceptor, HttpHandler).await?;

    Ok(())
}

struct HttpHandler;

impl Handler for HttpHandler {
    type Error<E>
    = Error<E>
    where
        E: Debug;

    async fn handle<T, const N: usize>(
        &self,
        _task_id: impl Display + Copy,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), Self::Error<T::Error>>
    where
        T: Read + Write,
    {
        let headers = conn.headers()?;

        if headers.method != Method::Get {
            conn.initiate_response(405, Some("Method Not Allowed"), &[])
                .await?;
        } else if headers.path != "/" {
            conn.initiate_response(404, Some("Not Found"), &[]).await?;
        } else {
            conn.initiate_response(200, Some("OK"), &[("Content-Type", "text/plain")])
                .await?;

            conn.write_all(b"Hello world!").await?;
        }

        Ok(())
    }
}