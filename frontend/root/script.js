const overlay = document.getElementById('overlay');
const closeBtn = document.getElementById('closeBtn');

window.addEventListener('DOMContentLoaded', () => {
    overlay.classList.add('active');
});

closeBtn.addEventListener('click', () => {
    overlay.classList.remove('active');
});