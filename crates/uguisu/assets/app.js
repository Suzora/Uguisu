// Small progressive enhancements. Every page also works without this file.
(function () {
  "use strict";

  document.addEventListener("click", function (event) {
    var button = event.target.closest("[data-copy]");
    if (!button) return;
    var field = document.querySelector(button.getAttribute("data-copy"));
    if (!field) return;
    var done = function () {
      var label = button.textContent;
      button.textContent = button.getAttribute("data-copied") || "Copied";
      setTimeout(function () { button.textContent = label; }, 1600);
    };
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(field.value).then(done);
    } else {
      field.select();
      document.execCommand("copy");
      done();
    }
  });

  document.addEventListener("change", function (event) {
    if (event.target.matches("[data-autosubmit]")) event.target.form.submit();
  });

  document.addEventListener("submit", function (event) {
    var message = event.target.getAttribute("data-confirm");
    if (message && !window.confirm(message)) event.preventDefault();
  });
})();
