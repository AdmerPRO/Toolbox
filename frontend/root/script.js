const overlay = document.getElementById("overlay");
const closeBtn = document.getElementById("closeBtn");

function getCookie(name) {
    const cookies = document.cookie.split(";");

    for (const cookie of cookies) {
        const [key, value] = cookie.trim().split("=");

        if (key === name) {
            return value;
        }
    }

    return null;
}

function setCookie(name, value, days) {
    const maxAge = days * 24 * 60 * 60;

    document.cookie = `${name}=${value}; Max-Age=${maxAge}; Path=/; SameSite=Lax`;
}

document.addEventListener("DOMContentLoaded", () => {
    if (!getCookie("popup_seen")) {
        overlay.classList.add("active");
    }
});

closeBtn.addEventListener("click", () => {
    overlay.classList.remove("active");
    setCookie("popup_seen", "1", 2);
});