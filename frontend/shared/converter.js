const form = document.querySelector('#convert-form');
const file = document.querySelector('#file');
const status = document.querySelector('#status');
const save = document.querySelector('#download-link');
const tool = document.body.dataset.tool;
const cutting = tool.startsWith('cut-');
const audio = ['audio', 'mute', 'gif'].includes(tool) || cutting;
const sourceType = document.querySelector('#source-type');
const preview = document.querySelector('#source-preview');
const imagePreview = document.querySelector('#image-preview');
const resultPreview = document.querySelector('#result-preview');
const resultImage = document.querySelector('#result-image');
let previewUrl;
let selectionPlaying = false;
const imageMode = () => tool === 'gif' && sourceType?.value === 'image';
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
    if (resultPreview) { resultPreview.pause(); resultPreview.removeAttribute('src'); resultPreview.load(); resultPreview.hidden = true; }
    if (resultImage) { resultImage.removeAttribute('src'); resultImage.hidden = true; }
});

form.addEventListener('submit', async event => {
    event.preventDefault();
    if (busy) return;
    save.hidden = true;
    const selected = file.files[0];
    if (!selected) { message('Choose a file first.', true); return; }
    const largeFile = audio && !imageMode();
    const maximum = (largeFile ? 200 : 20) * 1024 * 1024;
    if (!selected.size || selected.size > maximum) {
        message(`Choose a non-empty file up to ${largeFile ? 200 : 20} MiB.`, true); return;
    }
    const extension = selected.name.split('.').pop().toLowerCase();
    if (!(largeFile ? [tool === 'cut-mp3' ? 'mp3' : 'mp4'] : ['png', 'jpg', 'jpeg', 'webp', 'ico', 'bmp', 'tif', 'tiff', 'gif']).includes(extension)) {
        message('Choose a supported file type.', true); return;
    }
    const data = new FormData(form);
    let endpoint = form.action;
    if (imageMode()) {
        endpoint = '/api/convert/image';
        data.set('format', 'gif');
    }
    if (cutting) {
        const start = Number(document.querySelector('#start').value);
        const end = Number(document.querySelector('#end').value);
        if (!preview || !Number.isFinite(preview.duration) || !Number.isFinite(start) || !Number.isFinite(end) || start < 0 || end <= start || end > preview.duration + 0.05 || end > 7200) {
            message('Choose a valid start and end within the recording (up to 2 hours).', true); return;
        }
        endpoint += `?start=${start}&end=${end}`;
        preview.pause();
    }
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
    message(cutting ? 'Uploading and cutting your recording...' : tool === 'gif' ? 'Uploading and creating your GIF...' : tool === 'mute' ? 'Uploading and removing audio...' : audio ? 'Uploading and extracting audio...' : 'Uploading and processing your image...', false, true);
    try {
        const response = await fetch(endpoint, { method: 'POST', body: data });
        if (!response.ok) throw new Error(await response.text() || 'Conversion failed. Please try again.');
        const result = await response.json();
        save.href = result.download_url;
        save.hidden = false;
        if (resultPreview) { resultPreview.src = result.download_url; resultPreview.hidden = false; }
        if (resultImage) { resultImage.src = result.download_url; resultImage.hidden = false; }
        message('Your file is ready. Save it below. This link is available for 7 days.');
    } catch (error) {
        message(error.message || 'The request failed. Please try again.', true);
    } finally {
        busy = false;
        form.setAttribute('aria-busy', 'false');
        controls.forEach(control => { control.disabled = false; });
    }
});

function clearPreview() {
    selectionPlaying = false;
    if (preview) { preview.pause(); preview.removeAttribute('src'); preview.load(); preview.hidden = true; }
    if (imagePreview) { imagePreview.removeAttribute('src'); imagePreview.hidden = true; }
    if (previewUrl) { URL.revokeObjectURL(previewUrl); previewUrl = undefined; }
}
sourceType?.addEventListener('change', () => {
    clearPreview();
    file.value = '';
    file.accept = imageMode() ? '.png,.jpg,.jpeg,.webp,.ico,.bmp,.tif,.tiff,.gif' : '.mp4,video/mp4';
});
file.addEventListener('change', () => {
    clearPreview();
    const selected = file.files[0];
    if (!selected || !preview) return;
    previewUrl = URL.createObjectURL(selected);
    if (imageMode()) { imagePreview.src = previewUrl; imagePreview.hidden = false; }
    else { preview.src = previewUrl; preview.hidden = false; }
    if (cutting) { document.querySelector('#start').value = '0'; document.querySelector('#end').value = ''; }
});
preview?.addEventListener('loadedmetadata', () => {
    if (cutting && Number.isFinite(preview.duration)) {
        document.querySelector('#end').value = String(Math.floor(Math.min(preview.duration, 7200) * 1000) / 1000);
    }
});
preview?.addEventListener('error', () => message('This browser cannot preview the selected recording. Try a supported MP4 or MP3.', true));
for (const boundary of ['start', 'end']) {
    document.querySelector(`#set-${boundary}`)?.addEventListener('click', () => {
        if (!Number.isFinite(preview.duration)) { message('Choose a playable recording first.', true); return; }
        document.querySelector(`#${boundary}`).value = preview.currentTime.toFixed(3);
        save.hidden = true;
    });
}
document.querySelector('#play-selection')?.addEventListener('click', async () => {
    const start = Number(document.querySelector('#start').value);
    const end = Number(document.querySelector('#end').value);
    if (!Number.isFinite(preview.duration) || start < 0 || end <= start || end > preview.duration + 0.05) {
        message('Choose a valid fragment first.', true); return;
    }
    preview.currentTime = start;
    selectionPlaying = true;
    try { await preview.play(); } catch { selectionPlaying = false; message('The browser could not play this recording.', true); }
});
preview?.addEventListener('timeupdate', () => {
    if (selectionPlaying && preview.currentTime >= Number(document.querySelector('#end').value)) { preview.pause(); selectionPlaying = false; }
});
window.addEventListener('pagehide', clearPreview);
