//! Own the browser history listener; page models contain no DOM or JS callbacks.
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

use crate::{
    application::{History, Navigation},
    routing::{Page, Route},
};

pub(super) struct NavigationBridge {
    window: web_sys::Window,
    initial: Route,
    pending: Rc<RefCell<Option<Result<Route, String>>>>,
    popstate: Closure<dyn FnMut(web_sys::Event)>,
}

impl NavigationBridge {
    pub fn install(context: egui::Context) -> Result<Self, JsValue> {
        let window =
            web_sys::window().ok_or_else(|| JsValue::from_str("Browser window is unavailable"))?;
        let initial = location(&window)?;
        let pending = Rc::new(RefCell::new(None));
        let events = Rc::clone(&pending);
        let event_window = window.clone();
        let popstate = Closure::new(move |_: web_sys::Event| {
            *events.borrow_mut() =
                Some(location(&event_window).map_err(|error| format!("{error:?}")));
            context.request_repaint();
        });
        let bridge = Self {
            window,
            initial,
            pending,
            popstate,
        };
        bridge.window.add_event_listener_with_callback(
            "popstate",
            bridge.popstate.as_ref().unchecked_ref(),
        )?;
        bridge.title(bridge.initial.page);
        Ok(bridge)
    }

    pub fn initial(&self) -> Route {
        self.initial.clone()
    }

    pub fn take(&self) -> Option<Result<Route, String>> {
        self.pending.borrow_mut().take()
    }

    pub fn apply(&self, navigation: Navigation) -> Result<(), JsValue> {
        let Some(url) = navigation.route.url() else {
            return Ok(());
        };
        let location = self.window.location();
        let current = format!("{}{}", location.pathname()?, location.search()?);
        if current != url {
            let history = self.window.history()?;
            match navigation.history {
                History::Push => history.push_state_with_url(&JsValue::NULL, "", Some(&url))?,
                History::Replace => {
                    history.replace_state_with_url(&JsValue::NULL, "", Some(&url))?
                }
            }
        }
        self.title(navigation.route.page);
        Ok(())
    }

    pub fn title(&self, page: Page) {
        if let Some(document) = self.window.document() {
            document.set_title(match page {
                Page::Analysis => "EVM Abstract Lab · Analyzer",
                Page::Decompiler => "EVM Abstract Lab · Decompiler",
            });
        }
    }
}

fn location(window: &web_sys::Window) -> Result<Route, JsValue> {
    let location = window.location();
    Ok(Route::parse(&location.pathname()?, &location.search()?))
}

impl Drop for NavigationBridge {
    fn drop(&mut self) {
        let _ = self.window.remove_event_listener_with_callback(
            "popstate",
            self.popstate.as_ref().unchecked_ref(),
        );
    }
}
