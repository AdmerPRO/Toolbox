const form = document.querySelector('#convert-form');
const file = document.querySelector('#file');
const status = document.querySelector('#status');
const save = document.querySelector('#download-link');
const tool = document.body.dataset.tool;
const audio = ['audio', 'mute'].includes(tool);
let busy = false;

function message(text, error = false, loading = false) {
    status.textContent = text;
    status.classList.toggle('error', error);
    status.classList.toggle('busy', loading);
}

form.addEventListener('change', () => {
    save.hidden = true;
    save.removeAttribute('href');
    message('');
});

form.addEventListener('submit', async event => {
    event.preventDefault();
    if (busy) return;
    save.hidden = true;
    const selected = file.files[0];
    if (!selected) { message('Choose a file first.', true); return; }
    const maximum = (audio ? 200 : 20) * 1024 * 1024;
    if (!selected.size || selected.size > maximum) {
        message(`Choose a non-empty file up to ${audio ? 200 : 20} MiB.`, true); return;
    }
    const extension = selected.name.split('.').pop().toLowerCase();
    if (!(audio ? ['mp4'] : ['png', 'jpg', 'jpeg', 'webp', 'ico', 'bmp', 'tif', 'tiff', 'gif']).includes(extension)) {
        message('Choose a supported file type.', true); return;
    }
    const data = new FormData(form);
    let endpoint = form.action;
    if (tool === 'resize') {
        const width = Number(document.querySelector('#width').value);
        const height = Number(document.querySelector('#height').value);
        if (![width, height].every(value => Number.isInteger(value) && value >= 1 && value <= 4096)) {
            message('Choose width and height between 1 and 4096 pixels.', true); return;
        }
        data.delete('width');
        data.delete('height');
        endpoint += `?width=${width}&height=${height}`;
    }
    busy = true;
    form.setAttribute('aria-busy', 'true');
    const controls = [...form.querySelectorAll('input, select, button')];
    controls.forEach(control => { control.disabled = true; });
    message(tool === 'mute' ? 'Uploading and removing audio...' : audio ? 'Uploading and extracting audio...' : 'Uploading and processing your image...', false, true);
    try {
        const response = await fetch(endpoint, { method: 'POST', body: data });
        if (!response.ok) throw new Error(await response.text() || 'Conversion failed. Please try again.');
        const result = await response.json();
        save.href = result.download_url;
        save.hidden = false;
        message('Your file is ready. Save it below. This link is available for 7 days.');
    } catch (error) {
        message(error.message || 'The request failed. Please try again.', true);
    } finally {
        busy = false;
        form.setAttribute('aria-busy', 'false');
        controls.forEach(control => { control.disabled = false; });
    }
});
