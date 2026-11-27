/* Runs only inside an isolated preview. It has no Nexus IPC, credentials or app state. */
(() => {
  const logs = [];
  let picked = null;
  let picking = false;
  const add = (level, parts) => {
    logs.push({ level, text: parts.map((part) => { try { return typeof part === 'string' ? part : JSON.stringify(part); } catch { return String(part); } }).join(' ').slice(0, 4000) });
    if (logs.length > 100) logs.shift();
  };
  for (const level of ['log', 'info', 'warn', 'error']) {
    const original = console[level].bind(console);
    console[level] = (...parts) => { add(level, parts); original(...parts); };
  }
  addEventListener('error', (event) => add('error', [event.message, event.filename, event.lineno]));
  addEventListener('unhandledrejection', (event) => add('error', [String(event.reason)]));
  const selector = (element) => {
    const path = [];
    while (element && element !== document.body) {
      if (element.id && document.querySelectorAll('#' + CSS.escape(element.id)).length === 1) {
        path.unshift('#' + CSS.escape(element.id));
        return path.join(' > ');
      }
      const name = element.localName;
      if (!name) break;
      const siblings = [...(element.parentElement?.children || [])].filter((other) => other.localName === name);
      path.unshift(name + (siblings.length > 1 ? ':nth-of-type(' + (siblings.indexOf(element) + 1) + ')' : ''));
      element = element.parentElement;
    }
    return 'body > ' + path.join(' > ');
  };
  const visible = (element) => element.getClientRects().length > 0 && getComputedStyle(element).visibility === 'visible' && !element.closest('[inert],[aria-hidden=true]');
  const interactable = (element) => {
    if (!visible(element)) throw Error('The element is hidden. Read the page again.');
    if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true') throw Error('The element is disabled.');
  };
  const field = (element) => {
    const labels = [...(element.labels || [])].map((label) => { const copy = label.cloneNode(true); copy.querySelectorAll('input,select,textarea,button').forEach((control) => control.remove()); return copy.textContent.trim(); }).join(' ');
    const named = (element.getAttribute('aria-labelledby') || '').split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ').trim();
    const text = element.getAttribute('aria-label') || named || labels || element.innerText || element.getAttribute('placeholder') || '';
    return { selector: selector(element), text: text.slice(0, 200), tag: element.localName, type: element.type, disabled: element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true', readOnly: !!element.readOnly,
      ...(element instanceof HTMLInputElement && ['checkbox', 'radio'].includes(element.type) ? { checked: element.checked } : {}),
      ...((element instanceof HTMLInputElement && !['password', 'file'].includes(element.type)) || element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement ? { value: element.value.slice(0, 1000) } : {}),
      ...(element instanceof HTMLSelectElement ? { options: [...element.options].slice(0, 100).map((option) => ({ value: option.value, text: option.text, disabled: option.disabled || option.parentElement?.disabled === true })) } : {}) };
  };
  const click = (event) => {
    if (!picking) return;
    event.preventDefault(); event.stopImmediatePropagation();
    const element = event.target instanceof Element ? event.target : null;
    if (element) {
      const rect = element.getBoundingClientRect();
      picked = { selector: selector(element), text: (element.textContent || '').trim().slice(0, 1000), width: Math.round(rect.width), height: Math.round(rect.height) };
    }
    picking = false; document.documentElement.style.cursor = '';
  };
  addEventListener('click', click, true);
  addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      const wasPicking = picking; picking = false; document.documentElement.style.cursor = '';
      if (!wasPicking) location.href = 'nexus-preview-key://escape';
    } else if ((event.metaKey || event.ctrlKey) && event.shiftKey && !event.altKey && !event.isComposing && !event.repeat) {
      const action = { Backslash: 'toggle', KeyB: 'browser', KeyT: 'terminal' }[event.code];
      if (action) { event.preventDefault(); location.href = 'nexus-preview-key://' + action; }
    }
  }, true);
  Object.defineProperty(window, '__nxPreview', { value: {
    info() { const selection = picked; picked = null; return { url: location.href, title: document.title, logs: [...logs], picked: selection, picking }; },
    pick() { picking = true; picked = null; document.documentElement.style.cursor = 'crosshair'; },
    cancelPick() { picking = false; document.documentElement.style.cursor = ''; },
    read() { const elements = [...document.querySelectorAll('a,button,input,textarea,select,[role=button],[role=checkbox],[role=radio],[role=combobox],[role=textbox],[contenteditable=true]')].filter(visible); return { url: location.href, title: document.title, text: document.body?.innerText.slice(0, 16000) || '', elements: elements.slice(0, 100).map(field) }; },
    click(selector) { const element = document.querySelector(selector); if (!(element instanceof HTMLElement)) throw Error('Element not found'); interactable(element); element.scrollIntoView({ block: 'center' }); element.focus(); element.click(); return 'Clicked'; },
    type(selector, text) {
      const element = document.querySelector(selector);
      if (element instanceof HTMLElement && element.isContentEditable) {
        interactable(element); element.focus();
        const range = document.createRange(); range.selectNodeContents(element);
        const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
        if (!document.execCommand('insertText', false, text)) { element.textContent = text; element.dispatchEvent(new Event('input', { bubbles: true })); }
        return 'Typed';
      }
      if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement)) throw Error('Choose a text input or select field');
      interactable(element);
      if (element.readOnly) throw Error('The field is read-only.');
      let value = text;
      if (element instanceof HTMLSelectElement) {
        if (element.multiple) throw Error('Choose a single-selection field.');
        const byValue = [...element.options].find((option) => option.value === text);
        const byText = [...element.options].filter((option) => option.text.trim() === text);
        const option = byValue || (byText.length === 1 ? byText[0] : null);
        if (!option || option.disabled || option.parentElement?.disabled === true) throw Error('Choose an enabled option by its exact value or label.');
        value = option.value;
      }
      const proto = element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      element.focus();
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(element, value);
      element.dispatchEvent(new Event('input', { bubbles: true })); element.dispatchEvent(new Event('change', { bubbles: true }));
      return element instanceof HTMLSelectElement ? 'Selected' : 'Typed';
    }
  }});
})();
