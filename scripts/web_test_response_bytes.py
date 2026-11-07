"""Observe the app's body API without decoding its JSON in JavaScript."""


def install_byte_probe(page, padding_bytes=0):
    page.add_init_script("""(() => {
        const paddingBytes = PADDING;
        const originalFetch = window.fetch.bind(window);
        const synthetic = new WeakMap();
        const metrics = window.__responseBytesProbe = {text: 0, json: 0, arrays: []};
        const apiUrl = response => synthetic.get(response) || response.url;
        for (const method of ['text', 'json']) {
            const original = Response.prototype[method];
            Response.prototype[method] = function(...args) {
                if (apiUrl(this).includes('/api/')) {
                    metrics[method]++;
                    throw new Error('API responses must reach Wasm as bytes: ' + method);
                }
                return original.apply(this, args);
            };
        }
        const arrayBuffer = Response.prototype.arrayBuffer;
        Response.prototype.arrayBuffer = async function(...args) {
            const body = await arrayBuffer.apply(this, args);
            if (apiUrl(this).includes('/api/')) {
                metrics.arrays.push({url: apiUrl(this), status: this.status, bytes: body.byteLength});
            }
            return body;
        };
        let padded = false;
        window.fetch = async (...args) => {
            const response = await originalFetch(...args);
            const url = response.url;
            if (!paddingBytes || padded || !url.includes('/api/decompile/tasks/') || !url.endsWith('/result') || !response.ok) {
                return response;
            }
            padded = true;
            const reader = response.body.getReader();
            let remaining = paddingBytes;
            const spaces = new Uint8Array(1024 * 1024).fill(32);
            const body = new ReadableStream({
                async pull(controller) {
                    if (remaining) {
                        const size = Math.min(remaining, spaces.byteLength);
                        remaining -= size;
                        controller.enqueue(spaces.subarray(0, size));
                        return;
                    }
                    const chunk = await reader.read();
                    if (chunk.done) controller.close();
                    else controller.enqueue(chunk.value);
                },
                cancel(reason) { return reader.cancel(reason); }
            });
            // fetch already decoded the original HTTP content. Padding is legal
            // JSON whitespace, generated as bytes rather than one large string.
            const headers = new Headers(response.headers);
            headers.delete('content-encoding');
            headers.delete('content-length');
            const wrapped = new Response(body, {status: response.status, headers});
            synthetic.set(wrapped, url);
            return wrapped;
        };
    })();""".replace("PADDING", str(padding_bytes)))


def byte_probe_result(page):
    metrics = page.evaluate("window.__responseBytesProbe")
    assert metrics["text"] == metrics["json"] == 0, metrics
    assert metrics["arrays"], "the app never read an API response as bytes"
    return metrics
