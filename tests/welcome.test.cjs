const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../frontend/root/script.js'), 'utf8');

function page(cookie) {
    const nodes = {};
    for (const id of ['overlay', 'closeBtn', 'welcome-button', 'tool-guide', 'guide-link', 'image']) {
        nodes[id] = { open: false, hidden: false, listeners: {},
            addEventListener(name, fn) { this.listeners[name] = fn; },
            showModal() { assert.equal(this.open, false); this.open = true; },
            close() { this.open = false; this.listeners.close?.(); },
            querySelector() { return nodes.image; } };
    }
    const events = {};
    const document = { cookie, getElementById: id => nodes[id], querySelector: () => nodes['guide-link'] };
    vm.runInNewContext(source, { document, window: {
        addEventListener(name, fn, options) { events[name] = { fn, options }; }
    } });
    return { nodes, document, events };
}

test('welcome waits for privacy acceptance on first visit', () => {
    const p = page('');
    assert.equal(p.nodes.overlay.open, false);
    assert.equal(p.events['toolbox:privacy-accepted'].options.once, true);
    p.events['toolbox:privacy-accepted'].fn();
    assert.equal(p.nodes.overlay.open, true);
});

test('returning user sees welcome only when the seen cookie is absent', () => {
    assert.equal(page('privacy_policy=2026-10-03').nodes.overlay.open, true);
    assert.equal(page('privacy_policy=2026-10-03; popup_seen_v2=1').nodes.overlay.open, false);
    assert.equal(page('privacy_policy=old').nodes.overlay.open, false);
});

test('closing remembers welcome for a day and footer can reopen it', () => {
    const p = page('privacy_policy=2026-10-03');
    p.nodes.closeBtn.listeners.click();
    assert.equal(p.nodes.overlay.open, false);
    assert.match(p.document.cookie, /^popup_seen_v2=1; Max-Age=86400; Path=\/; SameSite=Lax$/);
    p.nodes['welcome-button'].listeners.click();
    assert.equal(p.nodes.overlay.open, true);
    p.nodes['welcome-button'].listeners.click();
});

test('guide navigation expands details and broken welcome image is hidden', () => {
    const p = page('');
    p.nodes['guide-link'].listeners.click();
    assert.equal(p.nodes['tool-guide'].open, true);
    p.nodes.image.listeners.error({ target: p.nodes.image });
    assert.equal(p.nodes.image.hidden, true);
});
