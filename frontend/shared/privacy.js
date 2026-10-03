(() => {
    'use strict';
    const version = '2026-10-03';
    const accepted = () => document.cookie.split(';').some(cookie => cookie.trim() === `privacy_policy=${version}`);
    const dialog = document.querySelector('#privacy-dialog');
    const notice = document.querySelector('#privacy-notice');
    if (!dialog || !notice) return;
    const isPolicy = location.pathname === '/privacy/' || location.pathname === '/privacy';
    const tools = [...document.querySelectorAll('form input, form select, form button')];
    function update() {
        tools.forEach(control => { control.disabled = !accepted(); });
        notice.hidden = accepted();
    }
    function open() { if (!dialog.open) dialog.showModal(); }
    dialog.addEventListener('cancel', event => event.preventDefault());
    document.querySelector('#privacy-accept').addEventListener('click', () => {
        document.cookie = `privacy_policy=${version}; Max-Age=31536000; Path=/; SameSite=Lax${location.protocol === 'https:' ? '; Secure' : ''}`;
        if (!accepted()) {
            document.querySelector('#privacy-cookie-error').hidden = false;
            return;
        }
        dialog.close();
        update();
        window.dispatchEvent(new Event('toolbox:privacy-accepted'));
    });
    document.querySelector('#privacy-decline').addEventListener('click', () => { dialog.close(); update(); });
    document.querySelector('#privacy-review').addEventListener('click', open);
    document.addEventListener('submit', event => {
        if (!accepted()) { event.preventDefault(); event.stopImmediatePropagation(); open(); }
    }, true);
    // Never dismiss a dialog without an affirmative choice. The full policy stays accessible.
    update();
    if (!accepted() && !isPolicy) open();
})();
