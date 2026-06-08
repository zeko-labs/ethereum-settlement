const root = document.documentElement;
const themeButton = document.querySelector(".theme-toggle");
const mobileMenu = document.querySelector(".mobile-menu");
const sidebar = document.querySelector(".sidebar");
const toc = document.querySelector("#toc-nav");
const headings = [...document.querySelectorAll(".article h2, .article h3")];
const sidebarLinks = [...document.querySelectorAll(".sidebar a")];

const savedTheme = localStorage.getItem("docs-theme");
if (savedTheme === "dark" || (!savedTheme && matchMedia("(prefers-color-scheme: dark)").matches)) {
  root.dataset.theme = "dark";
}

themeButton?.addEventListener("click", () => {
  const dark = root.dataset.theme !== "dark";
  root.dataset.theme = dark ? "dark" : "light";
  localStorage.setItem("docs-theme", dark ? "dark" : "light");
});

mobileMenu?.addEventListener("click", () => sidebar?.classList.toggle("open"));
sidebarLinks.forEach((link) => link.addEventListener("click", () => sidebar?.classList.remove("open")));

headings.forEach((heading) => {
  const link = document.createElement("a");
  link.href = `#${heading.id}`;
  link.textContent = heading.textContent;
  link.className = heading.tagName === "H3" ? "level-3" : "level-2";
  toc?.appendChild(link);
});

const trackedLinks = [...sidebarLinks, ...document.querySelectorAll("#toc-nav a")];
const observer = new IntersectionObserver((entries) => {
  const visible = entries.find((entry) => entry.isIntersecting);
  if (!visible) return;
  trackedLinks.forEach((link) => link.classList.toggle("current", link.hash === `#${visible.target.id}`));
}, { rootMargin: "-18% 0px -72% 0px" });

headings.forEach((heading) => observer.observe(heading));
