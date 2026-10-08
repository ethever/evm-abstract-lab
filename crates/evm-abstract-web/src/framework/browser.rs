//! eframe and browser integration boundary. Framework-owned callbacks and JS
//! futures necessarily use dynamic dispatch here; the workspace and painter
//! widgets remain statically dispatched and contain no transport logic.

use std::sync::mpsc::{self, Receiver, Sender};

use evm_abstract_protocol::{API_PATH, AnalyzeReply, AnalyzeRequest};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::app::{TransportError, Workspace};

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
    results: Receiver<Result<AnalyzeReply, TransportError>>,
    sender: Sender<Result<AnalyzeReply, TransportError>>,
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(result) = self.results.try_recv() {
            self.workspace.receive(result);
        }
        let mut request = self.workspace.show(ui);
        if self.initial_request {
            self.initial_request = false;
            request = Some(self.workspace.begin_analysis());
        }
        if let Some(request) = request {
            let sender = self.sender.clone();
            let context = ui.ctx().clone();
            spawn_local(async move {
                let result = analyze(request).await;
                let _ = sender.send(result);
                context.request_repaint();
            });
        }
        // A live region also makes canvas-only loading/error states accessible
        // to assistive tools and browser smoke checks.
        let status = self.workspace.accessible_status();
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

async fn analyze(request: AnalyzeRequest) -> Result<AnalyzeReply, TransportError> {
    let body = serde_json::to_string(&request).map_err(TransportError::Encode)?;
    let options = web_sys::RequestInit::new();
    options.set_method("POST");
    options.set_mode(web_sys::RequestMode::SameOrigin);
    options.set_body(&JsValue::from_str(&body));
    let request =
        web_sys::Request::new_with_str_and_init(API_PATH, &options).map_err(browser_error)?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(browser_error)?;
    request
        .headers()
        .set("Accept", "application/json")
        .map_err(browser_error)?;
    let window = web_sys::window().ok_or(TransportError::MissingWindow)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(browser_error)?
        .dyn_into::<web_sys::Response>()
        .map_err(browser_error)?;
    let status = response.status();
    let body = JsFuture::from(response.text().map_err(browser_error)?)
        .await
        .map_err(browser_error)?
        .as_string()
        .ok_or(TransportError::NonTextResponse)?;
    let reply: AnalyzeReply =
        serde_json::from_str(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !response.ok() && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

fn browser_error(error: JsValue) -> TransportError {
    TransportError::Browser {
        message: error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}
