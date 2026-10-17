
const form = document.querySelector('#video-form');
const input = document.querySelector('#youtube-url');
const status = document.querySelector('#status');
const result = document.querySelector('#video-result');
const thumbnail = document.querySelector('#thumbnail');
const title = document.querySelector('#video-title');
const quality = document.querySelector('#quality');
const download = document.querySelector('#download-button');
const save = document.querySelector('#download-link');
const find = form.querySelector('button');
const author = document.querySelector('#is-author');
let audio = document.body.dataset.format === 'mp3';
const platform = document.body.dataset.platform;
const platforms = {
    youtube: { name: 'YouTube', hosts: ['youtube.com', 'www.youtube.com', 'm.youtube.com', 'youtu.be'] },
    instagram: { name: 'Instagram', hosts: ['instagram.com', 'www.instagram.com'] },
    tiktok: { name: 'TikTok', hosts: ['tiktok.com', 'www.tiktok.com', 'm.tiktok.com', 'vm.tiktok.com', 'vt.tiktok.com'] }
};
const provider = platforms[platform];
const formatButtons = [...document.querySelectorAll('[data-download-format]')];
let currentVideo;
function populateQuality() {
    if (!currentVideo) return;
    const choices = audio ? [128, 192, 256, 320] : currentVideo.qualities;
    quality.replaceChildren(...choices.map(value => {
        const option = document.createElement('option');
        option.value = value;
        option.textContent = audio ? `${value} kbps` : `${value}p`;
        return option;
    }));
    quality.value = audio ? '192' : String(choices.includes(1080) ? 1080 : choices[choices.length - 1]);
    download.disabled = busy || !choices.length;
    if (!choices.length) message('No video resolutions are available. Choose Audio / MP3.', true);
}
function setFormat(format) {
    audio = format === 'mp3';
    document.body.dataset.format = audio ? 'mp3' : 'mp4';
    formatButtons.forEach(button => button.setAttribute('aria-pressed', String(button.dataset.downloadFormat === format)));
    document.querySelector('label[for="quality"]').textContent = audio ? 'Audio bitrate' : 'Video resolution';
    download.textContent = audio ? 'Download MP3 ↓' : 'Download MP4 ↓';
    save.textContent = audio ? 'Save MP3 file ↓' : 'Save MP4 file ↓';
    save.hidden = true;
    save.removeAttribute('href');
    message('');
    populateQuality();
}
formatButtons.forEach(button => button.addEventListener('click', () => {
    if (busy || !currentVideo) return;
    const format = button.dataset.downloadFormat;
    setFormat(format);
}));
if (formatButtons.length) setFormat('mp4');
let selectedUrl = '';
let busy = false;
function message(text, error = false, loading = false) {
    status.textContent = text;
    status.classList.toggle('error', error);
    status.classList.toggle('busy', loading);
}
function lock(value) {
    busy = value;
    find.disabled = download.disabled = input.disabled = quality.disabled = author.disabled = value;
    form.setAttribute('aria-busy', String(value));
    formatButtons.forEach(button => { button.disabled = value; });
    if (!value && currentVideo) download.disabled = !audio && !currentVideo.qualities.length;
}
async function request(endpoint, data) {
    const response = await fetch(endpoint, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(data) });
    if (!response.ok) throw new Error(await response.text() || 'The request failed. Please try again.');
    return response.json();
}
input.addEventListener('input', () => {
    currentVideo = undefined;
    selectedUrl = '';
    result.hidden = save.hidden = true;
    message('');
});
author.addEventListener('change', () => {
    if (!author.checked) save.hidden = true;
});
thumbnail.addEventListener('error', () => { thumbnail.hidden = true; });
form.addEventListener('submit', async event => {
    event.preventDefault();
    if (busy) return;
    const url = input.value.trim();
    let parsed;
    try { parsed = new URL(url); } catch { message(`Enter a valid ${provider?.name || 'YouTube, Instagram or TikTok'} link.`, true); return; }
    const hosts = provider ? provider.hosts : Object.values(platforms).flatMap(value => value.hosts);
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || (parsed.port && parsed.port !== '443') || !hosts.includes(parsed.hostname)) {
        message(`Enter an HTTPS video link from ${provider?.name || 'YouTube, Instagram or TikTok'}.`, true); return;
    }
    result.hidden = save.hidden = true;
    selectedUrl = '';
    currentVideo = undefined;
    lock(true);
    message('Finding your video...', false, true);
    try {
        const video = await request('/api/youtube/info', { url });
        currentVideo = video;
        title.textContent = video.title;
        thumbnail.hidden = !video.thumbnail;
        if (video.thumbnail) thumbnail.src = video.thumbnail;
        else thumbnail.removeAttribute('src');
        selectedUrl = url;
        result.hidden = false;
        message('Ready. Choose your quality and download.');
        populateQuality();
    } catch (error) { message(error.message, true); }
    finally { lock(false); }
});
download.addEventListener('click', async () => {
    if (busy || !selectedUrl) return;
    if (!author.checked) { message('Confirm that you are the author of this material.', true); author.focus(); return; }
    lock(true);
    save.hidden = true;
    message(audio ? 'Preparing your MP3. This can take a few minutes...' : 'Preparing your MP4. This can take a few minutes...', false, true);
    try {
        const file = await request(audio ? '/api/youtube/download/mp3' : '/api/youtube/download', { url: selectedUrl, quality: Number(quality.value), is_author: author.checked });
        save.href = file.download_url;
        save.setAttribute('download', '');
        save.hidden = false;
        save.click();
        message('Your file is ready for 7 days. After that, it is archived in a ZIP file that is deleted 30 days after creation. If the download did not start, use the save button below.');
    } catch (error) { message(error.message, true); }
    finally { lock(false); }
});
