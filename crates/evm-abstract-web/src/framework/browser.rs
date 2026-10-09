//! eframe and browser integration boundary. Framework-owned callbacks and JS
//! futures necessarily use dynamic dispatch here; the workspace and painter
//! widgets remain statically dispatched and contain no transport logic.

use std::{
    cell::Cell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};

use evm_abstract_protocol::{
    API_PATH, AnalyzeReply, AnalyzeRequest, JobReply, RPC_PROVIDERS_PATH, RpcProvidersReply,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::app::{Command, JobOperation, Message, TransportError, Workspace};

/// Start the browser workspace on an existing canvas element.
#[wasm_bindgen]
pub async fn start(canvas_id: &str) -> Result<(), JsValue> {
    let window =
        web_sys::window().ok_or_else(|| JsValue::from_str("The browser window is unavailable"))?;
    if !window.is_secure_context() {
        return Err(JsValue::from_str(
            "WebGPU requires a secure context. Open this workspace over HTTPS or localhost.",
        ));
    }
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("The browser document is unavailable"))?;
    let canvas = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| JsValue::from_str("The workspace canvas is missing"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    // wgpu normally permits WebGL fallback on the web. This workspace requires
    // the browser's WebGPU API, so absence of a usable adapter is a startup error.
    setup.instance_descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
    let options = eframe::WebOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::WgpuConfiguration {
            wgpu_setup: setup.into(),
            ..eframe::WgpuConfiguration::default()
        },
        ..eframe::WebOptions::default()
    };
    eframe::WebRunner::new()
        .start(
            canvas,
            options,
            Box::new(|creation| {
                crate::palette::configure(&creation.egui_ctx);
                Ok(Box::new(BrowserApp::new()))
            }),
        )
        .await
}

struct BrowserApp {
    workspace: Workspace,
    results: Receiver<Message>,
    sender: Sender<Message>,
    initial_request: bool,
    last_status: String,
}

impl BrowserApp {
    fn new() -> Self {
        let (sender, results) = mpsc::channel();
        Self {
            workspace: Workspace::default(),
            results,
            sender,
            initial_request: true,
            last_status: String::new(),
        }
    }
}

impl eframe::App for BrowserApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        self.workspace.prepare_input(input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(result) = self.results.try_recv() {
            self.workspace.receive_message(result);
        }
        let mut request = self.workspace.show(ui);
        if self.initial_request {
            self.initial_request = false;
            request = request.or_else(|| self.workspace.startup_command());
        }
        for request in [request, self.workspace.rpc_provider_command()]
            .into_iter()
            .flatten()
        {
            let sender = self.sender.clone();
            let context = ui.ctx().clone();
            spawn_local(async move {
                let result = execute(request).await;
                let _ = sender.send(result);
                context.request_repaint();
            });
        }
        // A live region also makes canvas-only loading/error states accessible
        // to assistive tools and browser smoke checks.
        let status = format!(
            "{}; {}",
            self.workspace.accessible_status(),
            self.workspace.accessible_rpc_provider_status()
        );
        if status != self.last_status {
            if let Some(element) = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id("analysis-status"))
            {
                element.set_text_content(Some(&status));
            }
            self.last_status = status;
        }
    }
}

async fn execute(command: Command) -> Message {
    match command {
        Command::RpcProviders { generation } => Message::RpcProviders {
            generation,
            result: rpc_providers().await,
        },
        Command::Submit {
            generation,
            request,
        } => Message::Status {
            generation,
            operation: JobOperation::Submit,
            result: status("POST", API_PATH, Some(*request)).await,
        },
        Command::Poll { generation, id } => Message::Status {
            generation,
            operation: JobOperation::Poll,
            result: status("GET", &format!("{API_PATH}/{id}"), None).await,
        },
        Command::Cancel { generation, id } => Message::Status {
            generation,
            operation: JobOperation::Cancel,
            result: status("DELETE", &format!("{API_PATH}/{id}"), None).await,
        },
        Command::Result { generation, id } => Message::Result {
            generation,
            id,
            result: Box::new(result(&format!("{API_PATH}/{id}/result")).await),
        },
    }
}

async fn rpc_providers() -> Result<RpcProvidersReply, TransportError> {
    let (status, ok, body) = fetch("GET", RPC_PROVIDERS_PATH, None).await?;
    let reply: RpcProvidersReply =
        serde_json::from_str(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

async fn status(
    method: &str,
    path: &str,
    request: Option<AnalyzeRequest>,
) -> Result<JobReply, TransportError> {
    let (status, ok, body) = fetch(method, path, request).await?;
    let reply: JobReply =
        serde_json::from_str(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

async fn result(path: &str) -> Result<AnalyzeReply, TransportError> {
    let (status, ok, body) = fetch("GET", path, None).await?;
    let reply: AnalyzeReply =
        serde_json::from_str(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

async fn fetch(
    method: &str,
    path: &str,
    request: Option<AnalyzeRequest>,
) -> Result<(u16, bool, String), TransportError> {
    let options = web_sys::RequestInit::new();
    options.set_method(method);
    options.set_mode(web_sys::RequestMode::SameOrigin);
    let controller = web_sys::AbortController::new().map_err(browser_error)?;
    options.set_signal(Some(&controller.signal()));
    if let Some(request) = request {
        let body = serde_json::to_string(&request).map_err(TransportError::Encode)?;
        options.set_body(&JsValue::from_str(&body));
    }
    let request = web_sys::Request::new_with_str_and_init(path, &options).map_err(browser_error)?;
    if method == "POST" {
        request
            .headers()
            .set("Content-Type", "application/json")
            .map_err(browser_error)?;
    }
    request
        .headers()
        .set("Accept", "application/json")
        .map_err(browser_error)?;
    let window = web_sys::window().ok_or(TransportError::MissingWindow)?;
    let seconds = if path.ends_with("/result") { 60 } else { 15 };
    let timed_out = Rc::new(Cell::new(false));
    let elapsed = Rc::clone(&timed_out);
    let timeout = Closure::once(move || {
        elapsed.set(true);
        controller.abort();
    });
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            timeout.as_ref().unchecked_ref(),
            seconds * 1000,
        )
        .map_err(browser_error)?;
    let result = async {
        let response = JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(browser_error)?
            .dyn_into::<web_sys::Response>()
            .map_err(browser_error)?;
        let status = response.status();
        let ok = response.ok();
        let body = JsFuture::from(response.text().map_err(browser_error)?)
            .await
            .map_err(browser_error)?
            .as_string()
            .ok_or(TransportError::NonTextResponse)?;
        Ok((status, ok, body))
    }
    .await;
    window.clear_timeout_with_handle(timer);
    drop(timeout);
    if timed_out.get() {
        Err(TransportError::Timeout {
            seconds: seconds as u32,
        })
    } else {
        result
    }
}

fn browser_error(error: JsValue) -> TransportError {
    TransportError::Browser {
        message: error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}
