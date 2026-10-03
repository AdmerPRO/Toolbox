const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../frontend/shared/privacy.js'), 'utf8');

function page(cookie = '', pathname = '/images/') {
    const nodes = {};
    for (const id of ['privacy-dialog', 'privacy-notice', 'privacy-accept', 'privacy-decline', 'privacy-review', 'privacy-cookie-error']) {
        nodes[`#${id}`] = { hidden: true, open: false, listeners: {},
            addEventListener(name, action) { this.listeners[name] = action; },
            showModal() { this.open = true; }, close() { this.open = false; } };
    }
    const controls = [{ disabled: false }, { disabled: false }];
    const events = {};
    const dispatches = [];
    let savedCookie = cookie;
    let cookieWrite = '';
    const document = { querySelector: selector => nodes[selector], querySelectorAll: () => controls,
        addEventListener: (name, action) => { events[name] = action; } };
    Object.defineProperty(document, 'cookie', { get: () => savedCookie, set: value => { cookieWrite = value; savedCookie = value.split(';')[0]; } });
    vm.runInNewContext(source, { document, location: { pathname, protocol: 'https:' }, window: { dispatchEvent: event => dispatches.push(event.type) }, Event });
    return { nodes, controls, events, dispatches, cookie: () => savedCookie, cookieWrite: () => cookieWrite };
}

test('first visit blocks tools; declining does not store acceptance', () => {
    const p = page();
    assert.equal(p.nodes['#privacy-dialog'].open, true);
    assert.ok(p.controls.every(c => c.disabled));
    let prevented = false;
    p.nodes['#privacy-dialog'].listeners.cancel({ preventDefault() { prevented = true; } });
    assert.equal(prevented, true);
    p.nodes['#privacy-decline'].listeners.click();
    assert.equal(p.cookie(), '');
    assert.equal(p.nodes['#privacy-notice'].hidden, false);
    assert.ok(p.controls.every(c => c.disabled));
});

test('acceptance stores the current version and enables tools', () => {
    const p = page();
    p.nodes['#privacy-accept'].listeners.click();
    assert.equal(p.cookie(), 'privacy_policy=2026-10-03');
    assert.match(p.cookieWrite(), /SameSite=Lax; Secure/);
    assert.equal(p.nodes['#privacy-dialog'].open, false);
    assert.equal(p.nodes['#privacy-notice'].hidden, true);
    assert.ok(p.controls.every(c => !c.disabled));
    assert.deepEqual(p.dispatches, ['toolbox:privacy-accepted']);
    assert.equal(page(p.cookie()).nodes['#privacy-dialog'].open, false);
});

test('stale acceptance prompts again; full policy is readable without acceptance', () => {
    assert.equal(page('privacy_policy=old').nodes['#privacy-dialog'].open, true);
    const p = page('', '/privacy/');
    assert.equal(p.nodes['#privacy-dialog'].open, false);
    p.nodes['#privacy-review'].listeners.click();
    assert.equal(p.nodes['#privacy-dialog'].open, true);
});
