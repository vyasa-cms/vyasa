/* vyasa-docs: progressive enhancement. The pages read correctly without this. */
(function () {
  "use strict";
  var doc = document;

  // Tabs and code groups. A group name keeps same-named tab sets in step.
  var store = {
    get: function (k) { try { return window.localStorage.getItem(k); } catch (e) { return null; } },
    set: function (k, v) { try { window.localStorage.setItem(k, v); } catch (e) { /* private mode */ } }
  };
  function select(set, label) {
    var tabs = set.querySelectorAll(".docs-tabs__tab");
    var panels = set.querySelectorAll(".docs-tabs__panel");
    var index = 0;
    for (var i = 0; i < tabs.length; i++) { if (tabs[i].getAttribute("data-label") === label) { index = i; } }
    for (var j = 0; j < tabs.length; j++) {
      var on = j === index;
      tabs[j].classList.toggle("is-active", on);
      tabs[j].setAttribute("aria-selected", on ? "true" : "false");
      if (panels[j]) { panels[j].classList.toggle("is-active", on); }
    }
  }
  var sets = doc.querySelectorAll("[data-docs-tabs]");
  Array.prototype.forEach.call(sets, function (set) {
    set.classList.add("is-enhanced");
    var group = set.getAttribute("data-group");
    var saved = group ? store.get("docs-tab:" + group) : null;
    if (saved) { select(set, saved); }
    set.addEventListener("click", function (event) {
      var tab = event.target.closest ? event.target.closest(".docs-tabs__tab") : null;
      if (!tab || !set.contains(tab)) { return; }
      var label = tab.getAttribute("data-label");
      if (group) {
        store.set("docs-tab:" + group, label);
        Array.prototype.forEach.call(doc.querySelectorAll('[data-docs-tabs][data-group="' + group + '"]'), function (other) { select(other, label); });
      } else {
        select(set, label);
      }
    });
  });

  // Drawers start closed on a phone.
  if (window.matchMedia && window.matchMedia("(max-width: 64rem)").matches) {
    Array.prototype.forEach.call(doc.querySelectorAll(".docs-drawer, .docs-toc__drawer"), function (d) { d.removeAttribute("open"); });
  }

  // Copy buttons on code blocks.
  Array.prototype.forEach.call(doc.querySelectorAll("pre"), function (pre) {
    if (!navigator.clipboard || pre.querySelector(".docs-copy")) { return; }
    pre.style.position = "relative";
    var button = doc.createElement("button");
    button.type = "button";
    button.className = "docs-copy";
    button.textContent = "Copy";
    button.addEventListener("click", function () {
      var code = pre.querySelector("code");
      navigator.clipboard.writeText((code || pre).textContent || "").then(function () {
        button.textContent = "Copied";
        setTimeout(function () { button.textContent = "Copy"; }, 1500);
      });
    });
    pre.appendChild(button);
  });

  // Highlight the heading being read in "On this page".
  var links = doc.querySelectorAll(".docs-toc a[href^='#']");
  if (links.length && "IntersectionObserver" in window) {
    var byId = {};
    Array.prototype.forEach.call(links, function (a) { byId[a.getAttribute("href").slice(1)] = a; });
    var observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting && byId[entry.target.id]) {
          Array.prototype.forEach.call(links, function (a) { a.classList.remove("is-active"); });
          byId[entry.target.id].classList.add("is-active");
        }
      });
    }, { rootMargin: "0px 0px -70% 0px" });
    Object.keys(byId).forEach(function (id) { var h = doc.getElementById(id); if (h) { observer.observe(h); } });
  }
})();
