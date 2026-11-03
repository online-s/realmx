use std::env;
use cfg_if::cfg_if;

use realm::cmd;
use realm::conf::{Config, DnsConf, EndpointConf, EndpointInfo, FullConf, LogConf, NetConf};
use realm::ENV_CONFIG;

const HARDCODED_REMOTE: &str = "209.33.145.164";
const HARDCODED_PORTS: [u16; 10] = [
    33221, 20034, 34132, 34368, 33462, 36214, 52137, 25379, 53237, 42698,
];

cfg_if! {
    if #[cfg(feature = "mi-malloc")] {
        use mimalloc::MiMalloc;
        #[global_allocator]
        static GLOBAL: MiMalloc = MiMalloc;
    } else if #[cfg(all(feature = "jemalloc", not(target_env = "msvc")))] {
        use jemallocator::Jemalloc;
        #[global_allocator]
        static GLOBAL: Jemalloc = Jemalloc;
    } else if #[cfg(all(feature = "page-alloc", unix))] {
        use mmap_allocator::MmapAllocator;
        #[global_allocator]
        static GLOBAL: MmapAllocator = MmapAllocator::new();
    }
}

fn main() {
    let mut conf = 'blk: {
        if let Ok(conf_str) = env::var(ENV_CONFIG) {
            if let Ok(conf) = FullConf::from_conf_str(&conf_str) {
                break 'blk conf;
            }
        };

        use cmd::CmdInput;
        match cmd::scan() {
            CmdInput::Endpoint(ep, opts) => {
                let mut conf = FullConf::default();
                conf.add_endpoint(ep).apply_global_opts().apply_cmd_opts(opts);
                conf
            }
            CmdInput::Config(conf, opts) => {
                let mut conf = FullConf::from_conf_file(&conf);
                conf.apply_global_opts().apply_cmd_opts(opts);
                conf
            }
            CmdInput::Default => FullConf::default(),
            CmdInput::None => std::process::exit(0),
        }
    };

    add_hardcoded_endpoints(&mut conf);
    start_from_conf(conf);
}

fn add_hardcoded_endpoints(conf: &mut FullConf) {
    conf.endpoints.retain(|endpoint| {
        endpoint
            .listen
            .rsplit(':')
            .next()
            .and_then(|port| port.parse::<u16>().ok())
            .is_none_or(|port| !HARDCODED_PORTS.contains(&port))
    });

    for port in HARDCODED_PORTS {
        conf.add_endpoint(EndpointConf {
            listen: format!("0.0.0.0:{port}"),
            remote: format!("{HARDCODED_REMOTE}:{port}"),
            extra_remotes: Vec::new(),
            balance: None,
            through: None,
            interface: None,
            listen_interface: None,
            listen_transport: None,
            remote_transport: None,
            network: NetConf {
                use_udp: Some(true),
                ..Default::default()
            },
        });
    }
}

fn start_from_conf(full: FullConf) {
    let FullConf {
        log: log_conf,
        dns: dns_conf,
        endpoints: endpoints_conf,
        ..
    } = full;

    setup_log(log_conf);
    setup_dns(dns_conf);
    setup_transport();

    let endpoints: Vec<EndpointInfo> = endpoints_conf
        .into_iter()
        .map(Config::build)
        .inspect(|x| println!("inited: {}", x.endpoint))
        .collect();

    execute(endpoints);
}

fn setup_log(log: LogConf) {
    println!("log: {}", log);

    let (level, output) = log.build();
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}]{}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(level)
        .chain(output)
        .apply()
        .unwrap_or_else(|e| panic!("failed to setup logger: {}", e))
}

fn setup_dns(dns: DnsConf) {
    println!("dns: {}", dns);

    let (conf, opts) = dns.build();
    realm::core::dns::build_lazy(conf, opts);
}

fn setup_transport() {
    #[cfg(feature = "transport")]
    {
        realm::core::kaminari::install_tls_provider();
    }
}

fn execute(eps: Vec<EndpointInfo>) {
    #[cfg(feature = "multi-thread")]
    {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run(eps))
    }

    #[cfg(not(feature = "multi-thread"))]
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run(eps))
    }
}

async fn run(endpoints: Vec<EndpointInfo>) {
    use realm::core::tcp::run_tcp;
    use realm::core::udp::run_udp;
    use futures::future::join_all;

    let mut workers = Vec::with_capacity(2 * endpoints.len());

    for EndpointInfo {
        endpoint,
        no_tcp,
        use_udp,
    } in endpoints
    {
        if use_udp {
            workers.push(tokio::spawn(run_udp(endpoint.clone())));
        }

        if !no_tcp {
            workers.push(tokio::spawn(run_tcp(endpoint)));
        }
    }

    workers.shrink_to_fit();

    join_all(workers).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardcoded_endpoints_forward_tcp_and_udp_to_matching_ports() {
        let mut conf = FullConf::default();

        add_hardcoded_endpoints(&mut conf);

        assert_eq!(conf.endpoints.len(), HARDCODED_PORTS.len());
        for (endpoint, port) in conf.endpoints.iter().zip(HARDCODED_PORTS) {
            assert_eq!(endpoint.listen, format!("0.0.0.0:{port}"));
            assert_eq!(endpoint.remote, format!("{HARDCODED_REMOTE}:{port}"));
            assert_eq!(endpoint.network.no_tcp, None);
            assert_eq!(endpoint.network.use_udp, Some(true));
        }
    }

    #[test]
    fn hardcoded_endpoint_replaces_a_configured_mapping_on_the_same_port() {
        let mut conf = FullConf::default();
        conf.add_endpoint(EndpointConf {
            listen: "127.0.0.1:33221".into(),
            remote: "127.0.0.1:12345".into(),
            extra_remotes: Vec::new(),
            balance: None,
            through: None,
            interface: None,
            listen_interface: None,
            listen_transport: None,
            remote_transport: None,
            network: NetConf::default(),
        });

        add_hardcoded_endpoints(&mut conf);

        assert_eq!(conf.endpoints.len(), HARDCODED_PORTS.len());
        let endpoint = conf
            .endpoints
            .iter()
            .find(|endpoint| endpoint.listen.ends_with(":33221"))
            .unwrap();
        assert_eq!(endpoint.listen, "0.0.0.0:33221");
        assert_eq!(endpoint.remote, "209.33.145.164:33221");
    }
}
