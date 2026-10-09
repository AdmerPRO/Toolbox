
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
const audio = document.body.dataset.format === 'mp3';
let selectedUrl = '';
let busy = false;
function message(text, error = false, loading = false) {
    status.textContent = text;
    status.classList.toggle('error', error);
    status.classList.toggle('busy', loading);
}
function lock(value) {
    busy = value;
    find.disabled = download.disabled = input.disabled = quality.disabled = value;
    form.setAttribute('aria-busy', String(value));
}
async function request(endpoint, data) {
    const response = await fetch(endpoint, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(data) });
    if (!response.ok) throw new Error(await response.text() || 'The request failed. Please try again.');
    return response.json();
}
input.addEventListener('input', () => {
    selectedUrl = '';
    result.hidden = save.hidden = true;
    message('');
});
thumbnail.addEventListener('error', () => { thumbnail.hidden = true; });
form.addEventListener('submit', async event => {
    event.preventDefault();
    if (busy) return;
    const url = input.value.trim();
    let parsed;
    try { parsed = new URL(url); } catch { message('Enter a valid YouTube, Instagram or TikTok link.', true); return; }
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || (parsed.port && parsed.port !== '443') || !['youtube.com', 'www.youtube.com', 'm.youtube.com', 'youtu.be', 'instagram.com', 'www.instagram.com', 'tiktok.com', 'www.tiktok.com', 'm.tiktok.com', 'vm.tiktok.com', 'vt.tiktok.com'].includes(parsed.hostname)) {
        message('Enter an HTTPS video link from YouTube, Instagram or TikTok.', true); return;
    }
    result.hidden = save.hidden = true;
    selectedUrl = '';
    lock(true);
    message('Finding your video...', false, true);
    try {
        const video = await request('/api/youtube/info', { url });
        const choices = audio ? [128, 192, 256, 320] : video.qualities;
        if (!choices.length) throw new Error('No video resolutions are available. Try the MP3 audio tool.');
        title.textContent = video.title;
        thumbnail.hidden = !video.thumbnail;
        if (video.thumbnail) thumbnail.src = video.thumbnail;
        else thumbnail.removeAttribute('src');
        quality.replaceChildren(...choices.map(value => {
            const option = document.createElement('option');
            option.value = value;
            option.textContent = audio ? `${value} kbps` : `${value}p`;
            return option;
        }));
        quality.value = audio ? '192' : String(choices.includes(1080) ? 1080 : choices[choices.length - 1]);
        selectedUrl = url;
        result.hidden = false;
        message('Ready. Choose your quality and download.');
    } catch (error) { message(error.message, true); }
    finally { lock(false); }
});
download.addEventListener('click', async () => {
    if (busy || !selectedUrl) return;
    lock(true);
    save.hidden = true;
    message(audio ? 'Preparing your MP3. This can take a few minutes...' : 'Preparing your MP4. This can take a few minutes...', false, true);
    try {
        const file = await request(audio ? '/api/youtube/download/mp3' : '/api/youtube/download', { url: selectedUrl, quality: Number(quality.value) });
        save.href = file.download_url;
        save.setAttribute('download', '');
        save.hidden = false;
        save.click();
        message('Your file is ready for 7 days. After that, it is archived in a ZIP file that is deleted 30 days after creation. If the download did not start, use the save button below.');
    } catch (error) { message(error.message, true); }
    finally { lock(false); }
});
