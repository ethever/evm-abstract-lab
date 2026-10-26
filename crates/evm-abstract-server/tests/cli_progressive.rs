//! The process-level policy must reach the same persistent presentation payer.

use evm_abstract_protocol as api;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(limit: u64) -> (Server, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_evm-abstract-server"))
        .args([
            "--bind",
            "127.0.0.1:0",
            "--assets",
            env!("CARGO_MANIFEST_DIR"),
            "--postprocess-work",
            &limit.to_string(),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut startup = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut startup)
        .unwrap();
    let base = startup
        .strip_prefix("EVM workbench: ")
        .and_then(|line| line.split_whitespace().next())
        .expect("server must report its actual bound address")
        .to_owned();
    (Server(child), base)
}

async fn terminal(client: &reqwest::Client, base: &str, id: api::JobId) -> api::JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = client
            .get(format!("{base}/api/tasks/{id}"))
            .send()
            .await
            .unwrap()
            .json::<api::JobReply>()
            .await
            .unwrap()
            .result
            .unwrap();
        if matches!(
            snapshot.state,
            api::JobState::Completed | api::JobState::Failed(_)
        ) {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "{snapshot:?}");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test]
async fn actual_server_cli_distinguishes_required_minimum_and_optional_rich_refusal() {
    let request = api::AnalyzeRequest::default();
    let calibration = evm_abstract_server::analyze::analyze(request.clone()).unwrap();
    let minimum = calibration.minimum.as_ref().unwrap();
    let required_work = minimum.export_usage.work + 2;
    assert!(calibration.postprocessing_work > required_work + 1);
    let optional_limit = required_work + (calibration.postprocessing_work - required_work) / 2;
    let client = reqwest::Client::new();
    for limit in [1, optional_limit] {
        let (_server, base) = start(limit);
        let job = client
            .post(format!("{base}/api/tasks"))
            .json(&request)
            .send()
            .await
            .unwrap()
            .json::<api::JobReply>()
            .await
            .unwrap()
            .result
            .unwrap();
        let snapshot = terminal(&client, &base, job.id).await;
        let bytes = client
            .get(format!("{base}/api/tasks/{}/result", job.id))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let reply: api::JobResultReply = serde_json::from_slice(&bytes).unwrap();
        if limit == 1 {
            let api::JobState::Failed(error) = snapshot.state else {
                panic!("required minimum failure must not become success or cancellation")
            };
            assert_eq!(error.code, api::ApiErrorCode::Export);
            assert!(snapshot.publication.is_none());
            assert_eq!(reply.result.unwrap_err(), error);
        } else {
            assert_eq!(snapshot.state, api::JobState::Completed);
            let result = reply.result.unwrap();
            assert_eq!(result.id, job.id);
            assert!(String::from_utf8_lossy(&bytes).contains(&format!("\"id\":{}", job.id)));
            assert_eq!(snapshot.publication.as_ref(), Some(&result.publication));
            assert_eq!(
                result.publication.version,
                api::PublicationVersion(api::DecimalU64(2))
            );
            assert_eq!(
                result.publication.minimum_version,
                Some(api::PublicationVersion(api::DecimalU64(1)))
            );
            assert!(result.publication.global_version.is_none());
            assert!(result.publication.native_version.is_none());
            assert!(result.publication.model_closed);
            assert!(matches!(
                result.report.rich_export,
                api::RichExportOutcome::Refused(_)
            ));
            assert!(result.report.postprocessing_work <= limit);
            let retained = result.report.minimum.as_ref().unwrap();
            assert_eq!(retained.postprocessing_max_work, limit);
            assert_eq!(retained.config, minimum.config);
            assert_eq!(retained.root_binding, minimum.root_binding);
            result.report.validate().unwrap();
        }
    }
    assert_eq!(request, api::AnalyzeRequest::default());
}
