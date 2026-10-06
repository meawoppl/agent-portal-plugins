/* Bench selection and viewer lifecycle; the diagram itself is upstream Visilog. */
"use strict";
const $ = (id) => document.getElementById(id);
let info = null,
  loading = false;

function error(message = "") {
  $("error").hidden = !message;
  $("error").textContent = message;
}
function empty(html) {
  $("empty").hidden = !html;
  $("frame").hidden = !!html;
  if (html) $("empty-text").innerHTML = html;
}
async function api(path, data) {
  const response = await fetch(
    path,
    data === undefined
      ? {}
      : {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-Visilog-Request": "1" },
          body: JSON.stringify(data),
        },
  );
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
  return result;
}
const key = () => "visilog:" + (info?.project || "") + ":test";

async function refresh() {
  info = await api("/api/state");
  const tests = info.tests.map((t) => t.id);
  const remembered = localStorage.getItem(key());
  $("test").replaceChildren(
    ...tests.map((id) => {
      const option = document.createElement("option");
      option.value = option.textContent = id;
      return option;
    }),
  );
  if (tests.includes(remembered)) $("test").value = remembered;
  error(info.error || "");
  $("load").disabled = !info.ok || !tests.length;
  if (!info.ok) {
    empty(
      "Visilog is not installed. Run <code>bin/visilog setup-visilog</code> (needs Cargo) and reload.",
    );
  } else if (!tests.length) {
    empty(
      "No testbenches found. Add <code>*_tb.v</code> files or a <code>.verilog-workbench.json</code>.",
    );
  } else if (!$("frame").hasAttribute("src")) {
    empty("Select a bench and press <strong>Load design</strong>.");
  }
  $("status").textContent = info.ok ? `visilog ${info.revision.slice(0, 10)}` : "";
}

async function open() {
  if (loading || !$("test").value) return;
  loading = true;
  $("load").disabled = $("test").disabled = true;
  error();
  $("status").textContent = "Elaborating…";
  try {
    const result = await api("/api/design/open", { test: $("test").value });
    localStorage.setItem(key(), result.test);
    $("frame").src = result.url;
    empty("");
    $("status").textContent = result.test;
  } catch (e) {
    error(e.message);
    $("status").textContent = "";
  } finally {
    loading = false;
    $("load").disabled = $("test").disabled = false;
  }
}

$("load").addEventListener("click", open);
$("test").addEventListener("change", () => {
  $("frame").removeAttribute("src");
  open();
});
refresh()
  .then(() => {
    if (info.ok && info.tests.length) open();
  })
  .catch((e) => error(e.message));
