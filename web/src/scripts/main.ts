// Progressive enhancement. The page is complete as static HTML; when the Rust
// API answers its health check, search and the contact form switch on.

const MARK_START = '\u0002';
const MARK_END = '\u0003';
const SEARCH_DEBOUNCE_MS = 160;
const HEALTH_TIMEOUT_MS = 2500;

interface Hit {
  anchor: string;
  section: string;
  title: string;
  snippet: string;
}

const apiBase = document.querySelector<HTMLMetaElement>('meta[name="api-base"]')?.content ?? '/';
const api = (path: string) => `${apiBase}api/${path}`;

async function apiAvailable(): Promise<boolean> {
  try {
    const res = await fetch(api('health'), { signal: AbortSignal.timeout(HEALTH_TIMEOUT_MS) });
    return res.ok && (await res.json()).status === 'ok';
  } catch {
    return false;
  }
}

async function errorMessage(res: Response, fallback: string): Promise<string> {
  try {
    const body = await res.json();
    return typeof body.error === 'string' ? body.error : fallback;
  } catch {
    return fallback;
  }
}

// Renders a server snippet without trusting it as HTML: split on control-char markers.
function renderSnippet(snippet: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  snippet.split(MARK_START).forEach((part, i) => {
    if (i === 0) {
      frag.append(part);
      return;
    }
    const [hit, rest = ''] = part.split(MARK_END);
    const mark = document.createElement('mark');
    mark.textContent = hit;
    frag.append(mark, rest);
  });
  return frag;
}

function setupAsk(): void {
  const form = document.querySelector<HTMLFormElement>('[data-ask-form]');
  const input = form?.querySelector<HTMLInputElement>('input');
  const status = document.querySelector<HTMLElement>('[data-ask-status]');
  const list = document.querySelector<HTMLOListElement>('[data-ask-results]');
  if (!form || !input || !status || !list) return;

  let timer: number | undefined;
  let inflight: AbortController | undefined;

  const run = async (query: string) => {
    inflight?.abort();
    if (!query.trim()) {
      list.replaceChildren();
      status.textContent = '';
      return;
    }
    const controller = (inflight = new AbortController());
    try {
      const res = await fetch(`${api('search')}?q=${encodeURIComponent(query)}`, {
        signal: controller.signal,
      });
      if (!res.ok) throw new Error(await errorMessage(res, 'Search failed.'));
      const { hits } = (await res.json()) as { hits: Hit[] };
      list.replaceChildren(
        ...hits.map((hit) => {
          const li = document.createElement('li');
          const link = document.createElement('a');
          link.href = `#${hit.anchor}`;
          const source = document.createElement('span');
          source.className = 'source';
          source.textContent = `${hit.section} · ${hit.title}`;
          const text = document.createElement('span');
          text.className = 'snippet';
          text.append(renderSnippet(hit.snippet));
          link.append(source, text);
          li.append(link);
          return li;
        }),
      );
      status.textContent = hits.length
        ? `${hits.length} ${hits.length === 1 ? 'passage' : 'passages'} cite “${query.trim()}”.`
        : `Nothing here mentions “${query.trim()}”. Try a tool name like LangGraph or Qdrant.`;
    } catch (err) {
      if (controller.signal.aborted) return;
      list.replaceChildren();
      status.textContent =
        err instanceof Error && err.message ? err.message : 'Search is unavailable right now.';
    }
  };

  input.addEventListener('input', () => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => run(input.value), SEARCH_DEBOUNCE_MS);
  });
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    window.clearTimeout(timer);
    void run(input.value);
  });
}

// Keeps aria-invalid in step with the visual :user-invalid state.
function syncAria(el: Element): void {
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    if (el.matches(':user-invalid')) el.setAttribute('aria-invalid', 'true');
    else el.removeAttribute('aria-invalid');
  }
}

function setupContact(): void {
  const form = document.querySelector<HTMLFormElement>('[data-contact-form]');
  const status = document.querySelector<HTMLElement>('[data-contact-status]');
  const button = form?.querySelector<HTMLButtonElement>('button[type="submit"]');
  if (!form || !status || !button) return;

  const CHECK_FIELDS = 'Check the highlighted fields.';
  for (const evt of ['blur', 'input'] as const) {
    form.addEventListener(evt, (e) => e.target instanceof Element && syncAria(e.target), true);
  }
  form.addEventListener('input', () => {
    if (status.textContent === CHECK_FIELDS && form.checkValidity()) status.textContent = '';
  });

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    form.classList.add('was-submitted');
    form.querySelectorAll('input, textarea').forEach(syncAria);
    if (!form.checkValidity()) {
      form.querySelector<HTMLElement>(':invalid')?.focus();
      status.textContent = CHECK_FIELDS;
      return;
    }
    const data = Object.fromEntries(new FormData(form)) as Record<string, string>;
    button.disabled = true;
    form.setAttribute('aria-busy', 'true');
    status.textContent = 'Sending…';
    try {
      const res = await fetch(api('contact'), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data),
      });
      if (!res.ok) {
        // Keep what the visitor typed; tell them what to fix.
        status.textContent = await errorMessage(res, 'Could not send. Please email me instead.');
        return;
      }
      form.reset();
      form.classList.remove('was-submitted');
      status.textContent = 'Sent. Thank you — I’ll reply by email.';
    } catch {
      status.textContent = 'Network error. Your message is still here; try again or email me.';
    } finally {
      button.disabled = false;
      form.removeAttribute('aria-busy');
    }
  });
}

void apiAvailable().then((ok) => {
  if (!ok) return;
  document.querySelectorAll<HTMLElement>('[data-needs-api]').forEach((el) => (el.hidden = false));
  setupAsk();
  setupContact();
});
