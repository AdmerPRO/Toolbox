const welcome = document.getElementById("overlay");
const closeButton = document.getElementById("closeBtn");
const welcomeButton = document.getElementById("welcome-button");
const cookieName = "popup_seen_v2";

function openWelcome() {
    if (!welcome.open) welcome.showModal();
}

function hasSeenWelcome() {
    return document.cookie.split(";").some(cookie => cookie.trim() === `${cookieName}=1`);
}

closeButton.addEventListener("click", () => welcome.close());
welcomeButton.addEventListener("click", openWelcome);
welcome.addEventListener("close", () => {
    document.cookie = `${cookieName}=1; Max-Age=86400; Path=/; SameSite=Lax`;
});
welcome.querySelector("img").addEventListener("error", event => {
    event.target.hidden = true;
});

function welcomeAfterPrivacy() {
    if (!hasSeenWelcome()) openWelcome();
}
if (document.cookie.split(';').some(cookie => cookie.trim() === 'privacy_policy=2026-10-03')) {
    welcomeAfterPrivacy();
} else {
    window.addEventListener('toolbox:privacy-accepted', welcomeAfterPrivacy, { once: true });
}
