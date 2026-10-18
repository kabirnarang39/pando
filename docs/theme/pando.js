// Keyboard and screen-reader fixes on top of mdBook's own script.
(function () {
  // The contents toggle is a <label>; make Enter and Space work on it like a button.
  const toggle = document.getElementById("mdbook-sidebar-toggle");
  if (toggle) {
    toggle.addEventListener("keydown", function (e) {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        toggle.click();
      }
    });
  }
  // Code blocks that scroll sideways must be reachable from the keyboard.
  document.querySelectorAll("pre > code").forEach(function (code) {
    if (code.scrollWidth > code.clientWidth) {
      code.setAttribute("tabindex", "0");
      code.setAttribute("role", "region");
      code.setAttribute("aria-label", "Code");
    }
  });
})();
