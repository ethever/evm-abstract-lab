//! Exercise real request admission, pinned acquisition and report projection.
use super::super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

struct Request {
    id: u64,
    method: String,
}
impl<'de> serde::Deserialize<'de> for Request {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Request;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("fixture JSON-RPC request")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Request, M::Error> {
                let mut id = None;
                let mut method = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => id = Some(map.next_value()?),
                        "method" => method = Some(map.next_value()?),
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Request {
                    id: id.ok_or_else(|| serde::de::Error::custom("missing id"))?,
                    method: method.ok_or_else(|| serde::de::Error::custom("missing method"))?,
                })
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
#[test]
fn rpc_report_distinguishes_pinned_header_from_execution_overrides() {
    thread::scope(|scope| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!(
            "http://{}/secret-path?key=secret-token",
            listener.local_addr().unwrap()
        );
        let providers = crate::rpc_providers::Registry::from_json(
            format!(
                r#"{{"providers":[{{"id":"fixture","name":"Fixture","endpoint":"{endpoint}"}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let (counts, completed) = mpsc::sync_channel(1);
        scope.spawn(move || {
            let mut count=0;
            while !stopped.load(Ordering::Acquire) {
                let (mut stream,_)=match listener.accept(){Ok(stream)=>stream,Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{thread::sleep(Duration::from_millis(1));continue;},Err(error)=>panic!("{error}")};
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut header=Vec::new();let mut byte=[0];while !header.ends_with(b"\r\n\r\n") {stream.read_exact(&mut byte).unwrap();header.push(byte[0]);}
                let header=String::from_utf8(header).unwrap();let length=header.lines().find_map(|line|{let(key,value)=line.split_once(':')?;key.eq_ignore_ascii_case("content-length").then(||value.trim().parse::<usize>().unwrap())}).unwrap();
                let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();let request:Request=serde_json::from_slice(&body).unwrap();
                let result=match request.method.as_str() {
                    "eth_chainId"=>"\"0x1\"".into(),
                    "eth_getBlockByNumber"|"eth_getBlockByHash"=>format!(r#"{{"hash":"0x{}","parentHash":"0x{}","number":"0x2a","timestamp":"0x1234","miner":"0x{}","mixHash":"0x{}","gasLimit":"0x1c9c380","baseFeePerGas":"0x7"}}"#,"11".repeat(32),"10".repeat(32),"33".repeat(20),"44".repeat(32)),
                    "eth_getCode"|"eth_getBalance"|"eth_getTransactionCount"=>{
                        let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("\"requireCanonical\":true"));assert!(body.contains(&"11".repeat(32)));
                        if request.method=="eth_getCode" {"\"0x43424600\"".into()}else{"\"0x0\"".into()}
                    },
                    other=>panic!("unexpected fixture method {other}"),
                };
                let reply=format!(r#"{{"jsonrpc":"2.0","id":{},"result":{result}}}"#,request.id);
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",reply.len()).unwrap();count+=1;
            }
            counts.send(count).unwrap();
        });
        let mut request = api::AnalyzeRequest {
            input: api::AnalysisInput::Rpc(api::RpcInput {
                provider_id: "fixture".into(),
                address: Address::repeat_byte(0x22).to_string(),
                block: api::BlockSelector::Latest,
                accounts: Vec::new(),
            }),
            ..api::AnalyzeRequest::default()
        };
        request.environment.number = Some("99".into());
        request.environment.chain_id = Some("33".into());
        let result = analyze_with_providers(request, &providers);
        stop.store(true, Ordering::Release);
        let requests = completed.recv().unwrap();
        let report = result.unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("secret-path"));
        assert!(!json.contains("secret-token"));
        assert!(!json.contains(&endpoint));
        let snapshot = report.metadata.snapshot.unwrap();
        assert_eq!(snapshot.chain_id, "0x1");
        assert_eq!(snapshot.number.as_deref(), Some("0x2a"));
        assert_eq!(snapshot.blob_base_fee, None);
        assert_eq!(
            report.metadata.environment.number.constants,
            Some(vec!["0x63".into()])
        );
        assert_eq!(
            report.metadata.environment.chain_id.constants,
            Some(vec!["0x21".into()])
        );
        assert_eq!(
            report.metadata.environment.timestamp.constants,
            Some(vec!["0x1234".into()])
        );
        assert!(
            report
                .metadata
                .environment
                .blob_base_fee
                .constants
                .is_none()
        );
        assert_eq!(report.acquisition.as_ref().unwrap().requests, requests);
        assert!(report.ssa.complete);
        assert_eq!(report.scope, api::AnalysisScope::RpcWorld);
    });
}
