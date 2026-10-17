const form = document.querySelector('#qr-form');
const text = document.querySelector('#qr-text');
const status = document.querySelector('#status');
const preview = document.querySelector('#qr-preview');
const save = document.querySelector('#download-link');
const button = form.querySelector('button[type=submit]');
const logo = document.querySelector('#qr-logo');
const logoSize = document.querySelector('#qr-logo-size');
const removeLogo = document.querySelector('#qr-remove-logo');
async function loadImage(blob) {
    const url = URL.createObjectURL(blob);
    try {
        const image = new Image();
        image.src = url;
        await image.decode();
        return image;
    } finally { URL.revokeObjectURL(url); }
}

async function customize(blob) {
    const source = await loadImage(blob);
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = source.width;
    const ctx = canvas.getContext('2d');
    ctx.drawImage(source, 0, 0);
    const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const rgb = selector => document.querySelector(selector).value.match(/[a-f\d]{2}/gi).map(value => parseInt(value, 16));
    const foreground = rgb('#qr-color');
    const background = rgb('#qr-background');
    for (let i = 0; i < pixels.data.length; i += 4) {
        pixels.data.set(pixels.data[i] < 128 ? foreground : background, i);
    }
    ctx.putImageData(pixels, 0, 0);
    const output = document.createElement('canvas');
    output.width = output.height = Number(document.querySelector('#qr-size').value);
    const target = output.getContext('2d');
    target.imageSmoothingEnabled = false;
    target.drawImage(canvas, 0, 0, output.width, output.height);
    if (logo.files[0]) {
        const image = await loadImage(logo.files[0]);
        const box = output.width * Number(logoSize.value) / 100;
        const padding = Math.ceil(output.width * 0.012);
        const start = (output.width - box) / 2;
        target.fillStyle = document.querySelector('#qr-background').value;
        target.fillRect(start - padding, start - padding, box + 2 * padding, box + 2 * padding);
        const scale = Math.min(box / image.naturalWidth, box / image.naturalHeight);
        const width = image.naturalWidth * scale;
        const height = image.naturalHeight * scale;
        target.imageSmoothingEnabled = true;
        target.drawImage(image, (output.width - width) / 2, (output.height - height) / 2, width, height);
    }
    const png = await new Promise(resolve => output.toBlob(resolve, 'image/png'));
    if (!png) throw new Error('Could not export the QR code.');
    return png;
}
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

form.addEventListener('input', () => {
    clearResult();
    status.textContent = '';
    document.querySelector('#qr-logo-size-value').value = `${logoSize.value}%`;
    removeLogo.hidden = !logo.files.length;
});
removeLogo.addEventListener('click', () => {
    logo.value = '';
    removeLogo.hidden = true;
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
    const file = logo.files[0];
    if (file && (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type) || file.size > 5 * 1024 * 1024)) {
        status.textContent = 'Choose a PNG, JPG or WebP image up to 5 MB.';
        status.classList.add('error');
        return;
    }
    if (document.querySelector('#qr-color').value === document.querySelector('#qr-background').value) {
        status.textContent = 'Choose different code and background colors.';
        status.classList.add('error');
        return;
    }
    busy = true;
    const options = form.querySelector('.qr-options');
    options.disabled = true;
    button.disabled = true;
    text.disabled = true;
    form.setAttribute('aria-busy', 'true');
    status.textContent = 'Generating your QR code...';
    try {
        const response = await fetch(form.action, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ text: text.value, high_correction: Boolean(file) })
        });
        if (!response.ok) throw new Error(file ? 'Could not generate this code. Try shorter text or remove the center image.' : await response.text() || 'QR generation failed.');
        objectUrl = URL.createObjectURL(await customize(await response.blob()));
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
        options.disabled = false;
        button.disabled = false;
        text.disabled = false;
        form.setAttribute('aria-busy', 'false');
    }
});
window.addEventListener('pagehide', clearResult);
