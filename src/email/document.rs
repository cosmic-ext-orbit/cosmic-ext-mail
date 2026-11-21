// SPDX-License-Identifier: GPL-3.0-only

use http::{HeaderMap, HeaderValue, header};
use servo::{
    ClipboardDelegate, LoadStatus, NavigationRequest, ServoDelegate, WebResourceLoad,
    WebResourceResponse, WebView, WebViewDelegate,
};
use std::cell::{Cell, RefCell};
use url::Url;

struct Document {
    url: Url,
    html: String,
}

pub(super) struct Delegate {
    document: RefCell<Document>,
    pub(super) frame_ready: Cell<bool>,
    pub(super) copied: RefCell<Option<String>>,
    pub(super) cursor: Cell<servo::Cursor>,
}

pub(super) struct BlockBackgroundNetwork;

impl ServoDelegate for BlockBackgroundNetwork {
    fn load_web_resource(&self, load: WebResourceLoad) {
        let url = load.request().url.as_url();
        if matches!(url.scheme(), "http" | "https" | "file") {
            let response = WebResourceResponse::new(url.clone());
            load.intercept(response).cancel();
        }
    }
}

impl WebViewDelegate for Delegate {
    fn request_navigation(&self, _: WebView, request: NavigationRequest) {
        if request.url == self.document.borrow().url {
            request.allow();
        } else {
            request.deny();
        }
    }

    fn load_web_resource(&self, _: WebView, load: WebResourceLoad) {
        let document = self.document.borrow();
        let request = load.request();
        let url = request.url.as_url();
        if !request.is_for_main_frame && url.scheme() == "data" {
            return;
        }
        let mut response = WebResourceResponse::new(url.clone());

        if !request.is_for_main_frame || *url != document.url {
            load.intercept(response).cancel();
            return;
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
            "default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; sandbox",
        ));
        headers.insert("x-dns-prefetch-control", HeaderValue::from_static("off"));
        response.headers = headers;
        load.intercept(response)
            .send_body_data(document.html.as_bytes().to_vec());
    }

    fn notify_new_frame_ready(&self, _: WebView) {
        self.frame_ready.set(true);
    }

    fn notify_load_status_changed(&self, _: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            self.frame_ready.set(true);
        }
    }

    fn notify_cursor_changed(&self, _: WebView, cursor: servo::Cursor) {
        self.cursor.set(cursor);
    }
}

impl ClipboardDelegate for Delegate {
    fn set_text(&self, _: WebView, text: String) {
        *self.copied.borrow_mut() = Some(text);
    }
}

impl Delegate {
    pub(super) fn new(url: Url, html: String) -> Self {
        Self {
            document: RefCell::new(Document { url, html }),
            frame_ready: Cell::new(false),
            copied: RefCell::new(None),
            cursor: Cell::new(servo::Cursor::default()),
        }
    }

    pub(super) fn replace_document(&self, url: Url, html: String) {
        *self.document.borrow_mut() = Document { url, html };
        self.frame_ready.set(false);
        self.copied.borrow_mut().take();
    }

    pub(super) fn document_is_ready(&self, webview: &WebView) -> bool {
        webview.url().as_ref() == Some(&self.document.borrow().url)
            && matches!(webview.load_status(), LoadStatus::Complete)
    }
}
