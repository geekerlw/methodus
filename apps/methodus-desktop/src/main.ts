import { listen } from '@tauri-apps/api/event';
import { isPermissionGranted, onAction, registerActionTypes, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';
import { api } from './api';
import { commandPaletteMarkup, goalDialogMarkup, learningDialogMarkup } from './dialogs';
import type { Dashboard, Goal, LibraryFilter, NodeDetails, Page, ReviewFilter, RunDetails, ViewState } from './types';
import { esc } from './ui/format';
import { icon } from './ui/icons';
import { renderPage, renderShell } from './views';
import './styles.css';

const app = document.querySelector<HTMLDivElement>('#app')!;

let data: Dashboard | null = null;
let page: Page = 'today';
let selectedRun: string | null = null;
let selectedNode: string | null = null;
let runDetails: RunDetails | null = null;
let query = '';
let reviewFilter: ReviewFilter = 'all';
let libraryFilter: LibraryFilter = 'all';
let busy = false;
let searchTimer: number | undefined;
const nodeDetails: Record<string, NodeDetails> = {};

let notificationReady: Promise<boolean> | undefined;
let notificationId = 1_000_000;
const notificationLinks = new Map<number, { page: Page; runId?: string }>();

function viewState(): ViewState {
  if (!data) throw new Error('dashboard is not loaded');
  return { data, page, query, selectedNode, selectedRun, reviewFilter, libraryFilter, runDetails, nodeDetails };
}

function render() {
  if (!data) return;
  const state = viewState();
  app.innerHTML = renderShell(state, renderPage(state));
  bindActions();
}

async function refresh() {
  try {
    data = await api.dashboard();
    if (page === 'run' && selectedRun) runDetails = await api.run(selectedRun);
    render();
  } catch (error) {
    app.innerHTML = `<main class="error-screen"><div class="error-mark">${icon('alert')}</div><h1>Methodus could not start</h1><p>${esc(error)}</p><p class="muted">Check METHODUS_HOME and the runtime installation, then relaunch.</p></main>`;
  }
}

function closeModal() { document.querySelector('#modal')?.remove(); }

function showToast(message: string, runId?: string) {
  document.querySelector('#toast')?.remove();
  const toast = document.createElement('button');
  toast.id = 'toast';
  toast.className = 'toast';
  toast.setAttribute('aria-live', 'polite');
  toast.setAttribute('aria-label', message);
  toast.innerHTML = `<span class="toast-dot"></span><span>${esc(message)}</span><span class="toast-arrow">${icon('chevron')}</span>`;
  toast.onclick = () => { toast.remove(); if (runId) void showRun(runId).catch((error) => showToast(`Could not load run: ${error}`)); };
  document.body.appendChild(toast);
  window.setTimeout(() => toast.remove(), 7000);
}

async function notifyDesktop(title: string, body: string, link?: { page: Page; runId?: string }) {
  notificationReady ??= (async () => {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === 'granted';
    return granted;
  })();
  if (!(await notificationReady)) return;
  const id = notificationId++;
  if (link) notificationLinks.set(id, link);
  await sendNotification({ id, title, body, actionTypeId: 'methodus-open' });
}

function openDialog(markup: string) {
  closeModal();
  document.body.insertAdjacentHTML('beforeend', markup);
  document.querySelectorAll('[data-close]').forEach((element) => element.addEventListener('click', closeModal));
}

function showLearningDialog() {
  openDialog(learningDialogMarkup());
  document.querySelector<HTMLFormElement>('#modal form')!.onsubmit = async (event) => {
    event.preventDefault();
    const form = new FormData(event.currentTarget as HTMLFormElement);
    closeModal();
    try { await api.startLearning(form.get('prompt'), form.get('runtime'), form.get('permission_mode')); await refresh(); }
    catch (error) { showToast(`Could not start runtime: ${error}`); }
  };
}

function showCommandPalette() {
  openDialog(commandPaletteMarkup());
  const filter = document.querySelector<HTMLInputElement>('#command-filter');
  const items = [...document.querySelectorAll<HTMLButtonElement>('[data-command]')];
  filter?.addEventListener('input', () => {
    const query = filter.value.trim().toLowerCase();
    items.forEach((item) => { item.hidden = query.length > 0 && !item.textContent?.toLowerCase().includes(query); });
  });
  items.forEach((item) => item.addEventListener('click', () => {
    closeModal();
    const command = item.dataset.command;
    if (command === 'new-learning') showLearningDialog();
    else if (command) { page = command as Page; selectedNode = null; runDetails = null; render(); }
  }));
}

function showGoalDialog(existing?: Goal) {
  openDialog(goalDialogMarkup(existing));
  document.querySelector<HTMLFormElement>('#modal form')!.onsubmit = async (event) => {
    event.preventDefault();
    const form = new FormData(event.currentTarget as HTMLFormElement);
    const input = {
      title: form.get('title'), prompt: form.get('prompt'),
      sources: String(form.get('sources') || '').split(/\r?\n/).map((source) => source.trim()).filter(Boolean),
      runtime: form.get('runtime'), permissionMode: form.get('permission_mode'), cadence: form.get('cadence'),
      reviewCadence: form.get('review_cadence'), summaryCadence: form.get('summary_cadence'), sourceCheckCadence: form.get('source_check_cadence'),
      quietHoursStart: String(form.get('quiet_hours_start') || '') || null, quietHoursEnd: String(form.get('quiet_hours_end') || '') || null,
      budgetUsd: Number(form.get('budget_usd') || 20), reviewPolicy: form.get('review_policy'), enabled: form.get('enabled') === 'on',
    };
    try { if (existing) await api.updateGoal(existing.id, input); else await api.saveGoal(input); closeModal(); page = 'goals'; await refresh(); }
    catch (error) { showToast(`Could not save goal: ${error}`); }
  };
}

async function showRun(runId: string) {
  runDetails = await api.run(runId);
  selectedRun = runId;
  page = 'run';
  selectedNode = null;
  render();
}

async function selectNode(id: string) {
  selectedNode = id;
  render();
  if (nodeDetails[id]) return;
  try { nodeDetails[id] = await api.node(id); render(); }
  catch (error) { showToast(`Could not load node detail: ${error}`); }
}

async function reviewAction(action: string, nodeId: string, targetId: string | null = null) {
  if (busy) return;
  busy = true;
  try { data = await api.reviewCandidate(nodeId, action, targetId, action === 'revalidate' ? 'revalidated from Methodus Desktop Review' : null); selectedNode = null; render(); }
  catch (error) { showToast(`Review action failed: ${error}`); }
  finally { busy = false; }
}

async function continueRun(prompt: string) {
  if (!selectedRun || !prompt.trim() || busy) return;
  busy = true;
  try { await api.continueLearning(selectedRun, prompt.trim(), runDetails?.attention?.id); await refresh(); }
  catch (error) { showToast(`Could not continue session: ${error}`); }
  finally { busy = false; }
}

function bindActions() {
  document.querySelectorAll<HTMLElement>('[data-page]').forEach((element) => element.addEventListener('click', () => { page = element.dataset.page as Page; if (page !== 'run') runDetails = null; selectedNode = null; render(); }));
  document.querySelectorAll<HTMLElement>('[data-run]').forEach((element) => element.addEventListener('click', () => void showRun(element.dataset.run!).catch((error) => showToast(`Could not load run: ${error}`))));
  document.querySelectorAll<HTMLElement>('[data-node]').forEach((element) => element.addEventListener('click', () => { if (page === 'sources') page = 'review'; void selectNode(element.dataset.node!); }));
  document.querySelectorAll<HTMLButtonElement>('[data-review-filter]').forEach((element) => element.addEventListener('click', () => { reviewFilter = element.dataset.reviewFilter as ReviewFilter; selectedNode = null; render(); }));
  document.querySelectorAll<HTMLButtonElement>('[data-library-filter]').forEach((element) => element.addEventListener('click', () => { libraryFilter = element.dataset.libraryFilter as LibraryFilter; selectedNode = null; render(); }));
  document.querySelectorAll<HTMLButtonElement>('[data-settings-message]').forEach((element) => element.addEventListener('click', () => showToast(element.dataset.settingsMessage || 'Settings are configured per Goal.')));
  document.querySelector<HTMLButtonElement>('#sidebar-new-learning')?.addEventListener('click', showLearningDialog);
  document.querySelector<HTMLButtonElement>('#new-learning')?.addEventListener('click', showLearningDialog);
  document.querySelector<HTMLButtonElement>('#new-goal')?.addEventListener('click', () => showGoalDialog());
  document.querySelectorAll<HTMLButtonElement>('[data-goal-edit]').forEach((element) => element.addEventListener('click', () => { const goal = data?.goals.find((item) => item.id === element.dataset.goalEdit); if (goal) showGoalDialog(goal); }));
  document.querySelectorAll<HTMLButtonElement>('[data-goal-delete]').forEach((element) => element.addEventListener('click', async () => { const goal = data?.goals.find((item) => item.id === element.dataset.goalDelete); if (!goal || !window.confirm(`Delete learning goal “${goal.title}”?`)) return; try { await api.deleteGoal(goal.id); await refresh(); } catch (error) { showToast(`Could not delete goal: ${error}`); } }));
  document.querySelectorAll<HTMLButtonElement>('[data-goal-toggle]').forEach((element) => element.addEventListener('click', async () => { try { await api.setGoalEnabled(element.dataset.goalToggle!, element.textContent === 'Resume'); await refresh(); } catch (error) { showToast(`Could not update goal: ${error}`); } }));
  document.querySelectorAll<HTMLButtonElement>('[data-goal-run]').forEach((element) => element.addEventListener('click', async () => { if (busy) return; busy = true; try { await api.runGoal(element.dataset.goalRun!); await refresh(); } catch (error) { showToast(`Could not start goal: ${error}`); } finally { busy = false; } }));
  document.querySelectorAll<HTMLButtonElement>('[data-review]').forEach((element) => element.addEventListener('click', () => void reviewAction(element.dataset.review!, element.dataset.nodeId!)));
  document.querySelectorAll<HTMLButtonElement>('[data-merge-node]').forEach((element) => element.addEventListener('click', () => { const target = window.prompt('Committed Knowledge node id to merge into:'); if (target?.trim()) void reviewAction('merge', element.dataset.mergeNode!, target.trim()); }));
  const followUp = document.querySelector<HTMLFormElement>('#follow-up');
  followUp?.addEventListener('submit', (event) => { event.preventDefault(); void continueRun(String(new FormData(followUp).get('prompt') || '')); });
  document.querySelector<HTMLInputElement>('#global-search')?.addEventListener('input', (event) => {
    const input = event.target as HTMLInputElement;
    query = input.value;
    if (page !== 'library' && query) { page = 'library'; render(); return; }
    window.clearTimeout(searchTimer);
    const caret = input.selectionStart ?? query.length;
    searchTimer = window.setTimeout(() => {
      render();
      const next = document.querySelector<HTMLInputElement>('#global-search');
      next?.focus();
      next?.setSelectionRange(caret, caret);
    }, 120);
  });
  document.querySelector<HTMLButtonElement>('#refresh')?.addEventListener('click', () => void refresh());
}

window.addEventListener('keydown', (event) => {
  const modifier = event.metaKey || event.ctrlKey;
  if (modifier && event.key.toLowerCase() === 'k') { event.preventDefault(); showCommandPalette(); }
  else if (modifier && event.key.toLowerCase() === 'n') { event.preventDefault(); showLearningDialog(); }
  else if (modifier && ['1', '2', '3', '4'].includes(event.key)) { event.preventDefault(); page = ({ '1': 'today', '2': 'goals', '3': 'review', '4': 'library' } as Record<string, Page>)[event.key]; selectedNode = null; render(); }
  else if (event.key === 'Escape') { if (document.querySelector('#modal')) closeModal(); else if (page === 'run') { page = 'today'; runDetails = null; render(); } }
});

void registerActionTypes([{ id: 'methodus-open', actions: [{ id: 'open', title: 'Open in Methodus' }] }]).catch(() => undefined);
void onAction((notification) => { const link = notification.id ? notificationLinks.get(notification.id) : undefined; if (!link) return; page = link.page; selectedRun = link.runId ?? null; if (link.runId) void showRun(link.runId).catch((error) => showToast(`Could not load run: ${error}`)); else void refresh(); });
void listen<{ id: string; run_id: string; title: string; prompt: string }>('attention-required', (event) => { showToast(event.payload.title, event.payload.run_id); void notifyDesktop('Methodus needs your input', event.payload.prompt, { page: 'run', runId: event.payload.run_id }); void refresh(); });
void listen<{ runId: string; count: number }>('review-ready', (event) => { showToast(`${event.payload.count} candidate${event.payload.count === 1 ? '' : 's'} ready for Review`, event.payload.runId); void notifyDesktop('Methodus candidate ready', `${event.payload.count} candidate${event.payload.count === 1 ? '' : 's'} are waiting for your Review decision.`, { page: 'run', runId: event.payload.runId }); void refresh(); });
void listen<{ goalId: string; message: string }>('scheduler-error', (event) => { showToast(`Scheduled learning failed: ${event.payload.message}`); void notifyDesktop('Methodus scheduler error', event.payload.message, { page: 'goals' }); void refresh(); });
void listen<{ goalId: string; spentUsd: number; budgetUsd: number }>('budget-exhausted', (event) => { showToast(`Goal budget reached: $${event.payload.spentUsd.toFixed(2)} / $${event.payload.budgetUsd.toFixed(2)}`); void notifyDesktop('Methodus budget reached', `Automatic work paused at $${event.payload.spentUsd.toFixed(2)} for this month.`, { page: 'goals' }); void refresh(); });
void listen<{ nodeIds: string[]; count: number }>('source-stale', (event) => { showToast(`${event.payload.count} source change${event.payload.count === 1 ? '' : 's'} need review`); void notifyDesktop('Methodus source change', `${event.payload.count} reviewed node${event.payload.count === 1 ? '' : 's'} became stale.`, { page: 'review' }); page = 'review'; void refresh(); });
void listen('run-updated', () => { if (!busy) void refresh(); });
void refresh();
