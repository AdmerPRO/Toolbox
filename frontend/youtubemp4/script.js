const form = document.querySelector("#video-form");
const urlInput = document.querySelector("#youtube-url");
const status = document.querySelector("#status");
const result = document.querySelector("#video-result");
const thumbnail = document.querySelector("#thumbnail");
const title = document.querySelector("#video-title");
const quality = document.querySelector("#quality");
const downloadButton = document.querySelector("#download-button");

function setStatus(message, isError = false) {
    status.textContent = message;
    status.classList.toggle("error", isError);
}

async function request(endpoint, body) {
    const response = await fetch(endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
    const data = response.headers.get("content-type")?.includes("application/json")
        ? await response.json()
        : await response.text();
    if (!response.ok) {
        throw new Error(typeof data === "string" ? data : "A server error occurred.");
    }
    return data;
}

form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const url = urlInput.value.trim();
    result.hidden = true;
    form.querySelector("button").disabled = true;
    setStatus("Loading video information…");

    try {
        const video = await request("/api/youtube/info", { url });
        thumbnail.src = video.thumbnail || "";
        thumbnail.hidden = !video.thumbnail;
        title.textContent = video.title;
        quality.replaceChildren(...video.qualities.map((height) => {
            const option = document.createElement("option");
            option.value = height;
            option.textContent = `${height}p`;
            return option;
        }));
        result.hidden = false;
        setStatus("");
    } catch (error) {
        setStatus(error.message, true);
    } finally {
        form.querySelector("button").disabled = false;
    }
});

downloadButton.addEventListener("click", async () => {
    downloadButton.disabled = true;
    setStatus("Cooking your video... pls wait it might take a while");

    try {
        const video = await request("/api/youtube/download", {
            url: urlInput.value.trim(),
            quality: Number(quality.value),
        });
        window.location.assign(video.download_url);
        setStatus("Your download has started.");
    } catch (error) {
        setStatus(error.message, true);
    } finally {
        downloadButton.disabled = false;
    }
});
