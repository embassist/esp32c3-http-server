//% FEATURES: embassy embassy-generic-timers esp-wifi esp-wifi/wifi esp-wifi/utils esp-wifi/sniffer esp-hal/unstable

#![no_std]
#![no_main]

use core::{net::Ipv4Addr, str::FromStr};

use embassy_executor::Spawner;
use embassy_net::{
    tcp::TcpSocket,
    IpListenEndpoint,
    Ipv4Cidr,
    Runner,
    Stack,
    StackResources,
    StaticConfigV4,
};
use embassy_time::{Duration, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, rng::Rng, timer::timg::TimerGroup, timer::systimer::SystemTimer};
use esp_println::{print, println};
use esp_wifi::{
    init,
    wifi::{
        AccessPointConfiguration,
        Configuration,
        WifiApDevice,
        WifiController,
        WifiDevice,
        WifiEvent,
        WifiState,
        // AuthMethod,
    },
    EspWifiController,
};

macro_rules! make_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const GATEWAY_IP: &'static str = "192.168.2.1";

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let mut rng = Rng::new(peripherals.RNG);

    let init = &*make_static!(
        EspWifiController<'static>,
        init(timg0.timer0, rng.clone(), peripherals.RADIO_CLK).unwrap()
    );

    let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(&init, wifi, WifiApDevice).unwrap();

    let systimer = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(systimer.alarm0);

    let gateway_ip = Ipv4Addr::from_str(GATEWAY_IP).unwrap();

    let config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(gateway_ip, 24),
        gateway: Some(gateway_ip),
        dns_servers: Default::default(),
    });

    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    let (stack, runner) = embassy_net::new(
        wifi_interface,
        config,
        make_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();
    spawner.spawn(run_dhcp(stack, GATEWAY_IP)).ok();

    let mut rx_buffer = [0; 1536];
    let mut tx_buffer = [0; 1536];

    while !stack.is_link_up() {
        Timer::after(Duration::from_millis(500)).await;
    }
    // println!("Connect to the AP `esp-wifi` and point your browser to http://{GATEWAY_IP}:8080/");
    // println!("DHCP is enabled so there's no need to configure a static IP, just in case:");
    while !stack.is_config_up() {
        Timer::after(Duration::from_millis(100)).await
    }
    // stack
    //     .config_v4()
    //     .inspect(|c| println!("ipv4 config: {c:?}"));

    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(10)));
    loop {
        println!("[HTTP] Listening...");
        if let Err(e) = socket
            .accept(IpListenEndpoint {
                addr: None,
                port: 8080,
            })
            .await
        {
            println!("[HTTP] Connection failed, since: {:?}", e);
            continue;
        }

        use embedded_io_async::Write;

        let mut buffer = [0u8; 1024];
        let mut pos = 0;
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) => {
                    println!("read EOF");
                    break;
                }
                Ok(len) => {
                    let to_print =
                        unsafe { core::str::from_utf8_unchecked(&buffer[..(pos + len)]) };

                    if to_print.contains("\r\n\r\n") {
                        print!("{}", to_print);
                        println!();
                        break;
                    }

                    pos += len;
                }
                Err(e) => {
                    println!("read error: {:?}", e);
                    break;
                }
            };
        }

        if let Err(e) = socket
            .write_all(
                b"HTTP/1.0 200 OK\r\n\r\n\
            <html>\
                <body>\
                    <h1>Hello Rust! Hello esp-wifi!</h1>\
                </body>\
            </html>\r\n\
            ",
            ).await
        {
            println!("write error: {:?}", e);
        }

        if let Err(e) = socket.flush().await {
            println!("flush error: {:?}", e);
        }
        Timer::after(Duration::from_millis(1000)).await;

        socket.close();
        Timer::after(Duration::from_millis(1000)).await;

        socket.abort();
    }
}

#[embassy_executor::task]
async fn run_dhcp(stack: Stack<'static>, gw_ip_addr: &'static str) {
    use core::net::{Ipv4Addr, SocketAddrV4};

    use edge_dhcp::{
        io::{self, DEFAULT_SERVER_PORT},
        server::{Server, ServerOptions},
    };
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let ip = Ipv4Addr::from_str(gw_ip_addr).unwrap();

    let mut buf = [0u8; 1500];

    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound_socket = Udp::new(stack, &buffers);
    let mut bound_socket = unbound_socket
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .unwrap();

    loop {
        _ = io::server::run(
            &mut Server::<64>::new(ip),
            &ServerOptions::new(ip, Some(&mut gw_buf)),
            &mut bound_socket,
            &mut buf,
        )
            .await
            .inspect_err(|e| log::warn!("[DHCP] Error: {e:?}"));
        Timer::after(Duration::from_millis(500)).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        match esp_wifi::wifi::wifi_state() {
            WifiState::ApStarted => {
                controller.wait_for_event(WifiEvent::ApStop).await;
                Timer::after(Duration::from_millis(5000)).await
            },
            _ => {}
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let config = Configuration::AccessPoint(AccessPointConfiguration {
                ssid: "HA:LED".try_into().unwrap(),
                ssid_hidden: true,
                // password: "cxllmerichie".try_into().unwrap(),
                // auth_method: AuthMethod::WPA2Personal,
                max_connections: 2u16,
                ..Default::default()
            });
            match controller.set_configuration(&config) {
                Ok(_) => println!("[WiFi] Config set."),
                Err(e) => println!("[WiFi] Configuration failed, since: {:?}", e),
            }
            match controller.start_async().await {
                Ok(_) => println!("[WiFi] Started."),
                Err(e) => println!("[WiFi] Error starting, since: {:?}", e),
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static, WifiApDevice>>) {
    runner.run().await
}

// let peripherals = esp_hal::init({
//     let mut config = esp_hal::Config::default();
//     config.cpu_clock = CpuClock::max();
//     config
// });
// peripherals.SYSTIMER;