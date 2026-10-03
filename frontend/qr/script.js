const form = document.querySelector('#qr-form');
const text = document.querySelector('#qr-text');
const status = document.querySelector('#status');
const preview = document.querySelector('#qr-preview');
const save = document.querySelector('#download-link');
const button = form.querySelector('button');
let objectUrl;
let busy = false;

function clearResult() {
    preview.hidden = true;
    preview.removeAttribute('src');
    save.hidden = true;
    save.removeAttribute('href');
    if (objectUrl) URL.revokeObjectURL(objectUrl);
    objectUrl = undefined;
}

text.addEventListener('input', () => {
    clearResult();
    status.textContent = '';
});

form.addEventListener('submit', async event => {
    event.preventDefault();
    if (busy) return;
    clearResult();
    status.classList.remove('error');
    if (!text.value.trim() || new TextEncoder().encode(text.value).length > 2000) {
        status.textContent = 'Enter text or a URL up to 2000 UTF-8 bytes.';
        status.classList.add('error');
        return;
    }
    busy = true;
    button.disabled = true;
    text.disabled = true;
    form.setAttribute('aria-busy', 'true');
    status.textContent = 'Generating your QR code...';
    try {
        const response = await fetch(form.action, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ text: text.value })
        });
        if (!response.ok) throw new Error(await response.text() || 'QR generation failed.');
        objectUrl = URL.createObjectURL(await response.blob());
        preview.src = objectUrl;
        preview.hidden = false;
        save.href = objectUrl;
        save.hidden = false;
        status.textContent = 'Your QR code is ready. Save it below.';
    } catch (error) {
        status.textContent = error.message || 'The request failed. Please try again.';
        status.classList.add('error');
    } finally {
        busy = false;
        button.disabled = false;
        text.disabled = false;
        form.setAttribute('aria-busy', 'false');
    }
});
window.addEventListener('pagehide', clearResult);
