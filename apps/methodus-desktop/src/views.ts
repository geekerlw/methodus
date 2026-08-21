import type { Attention, Dashboard, Goal, LibraryFilter, Node, NodeDetails, Page, ReviewFilter, Run, ViewState } from './types';
import { esc, humanStatus, relative, statusClass } from './ui/format';
import { icon } from './ui/icons';

export function empty(title: string, detail: string) {
  return `<div class="empty"><div class="empty-icon">${icon('spark')}</div><h3>${esc(title)}</h3><p>${esc(detail)}</p></div>`;
}

function navItem(page: Page, current: Page, label: string, count = '') {
  return `<button class="nav-item ${current === page ? 'active' : ''}" data-page="${page}" aria-current="${current === page ? 'page' : 'false'}"><span class="nav-label-content">${icon(page)}<span>${label}</span></span>${count ? `<b>${esc(count)}</b>` : ''}</button>`;
}

function pageTitle(state: ViewState) {
  if (state.page === 'run') return state.runDetails?.run.goal || 'Run';
  return ({ today: 'Today', goals: 'Learning goals', review: 'Review inbox', library: 'Library', sources: 'Sources', settings: 'Settings' } as Record<string, string>)[state.page] ?? 'Today';
}

export function renderShell(state: ViewState, content: string) {
  const { data, page } = state;
  return `<div class="shell">
    <aside class="sidebar">
      <div class="brand"><span class="brand-mark">${icon('methodus')}</span><div><strong>Methodus</strong><small>knowledge workbench</small></div></div>
      <button class="sidebar-new" id="sidebar-new-learning"><span>${icon('plus')}</span><span>New learning</span><kbd>⌘N</kbd></button>
      <div class="nav-label">Workspace</div>
      <nav>${navItem('today', page, 'Today', String(data.attentions.length + data.review_count || ''))}${navItem('goals', page, 'Learning goals', String(data.goals.length || ''))}${navItem('review', page, 'Review', String(data.review_count || ''))}${navItem('library', page, 'Library')}${navItem('sources', page, 'Sources')}</nav>
      <div class="sidebar-sessions"><div class="nav-label">Recent sessions</div>${data.runs.slice(0, 4).map(sidebarRun).join('') || '<div class="sidebar-empty">No sessions yet</div>'}</div>
      <div class="sidebar-bottom"><button class="nav-item ${page === 'settings' ? 'active' : ''}" data-page="settings" aria-current="${page === 'settings' ? 'page' : 'false'}"><span class="nav-label-content">${icon('settings')}<span>Settings</span></span></button><div class="home-path" title="Methodus home">${esc(data.home)}</div></div>
    </aside>
    <section class="workspace">
      <header class="topbar" data-tauri-drag-region><div class="crumb" data-tauri-drag-region>${page === 'run' ? `<button class="back-button" data-page="today" aria-label="Back to Today">${icon('chevron')}<span>Today</span></button><i>/</i>` : '<span>Methodus</span><i>/</i>'}<strong>${esc(pageTitle(state))}</strong></div><label class="search"><span>⌘ K</span><input id="global-search" aria-label="Search knowledge, goals, and runs" value="${esc(state.query)}" placeholder="Search knowledge, goals, runs" /></label><div class="runtime-pill"><span class="pulse"></span> ${data.active_runs.length ? `${data.active_runs.length} runtime${data.active_runs.length > 1 ? 's' : ''} running` : 'Runtimes ready'}</div><button class="icon-button" id="refresh" title="Refresh dashboard" aria-label="Refresh dashboard">${icon('refresh')}</button></header>
      <main class="main-content">${content}</main>
    </section>
  </div>`;
}

function sidebarRun(run: Run) {
  return `<button class="sidebar-session" data-run="${esc(run.run_id)}"><span class="sidebar-session-dot ${statusClass(run.status)}"></span><span><strong>${esc(run.goal)}</strong><small>${esc(run.runtime)} · ${relative(run.updated_at)}</small></span></button>`;
}

function attentionCard(attention: Attention, run: Run) {
  const permission = attention.kind === 'permission';
  return `<button class="attention-row" data-run="${esc(run.run_id)}"><span class="run-icon ${permission ? 'permission' : 'question'}">${icon(permission ? 'alert' : 'pulse')}</span><span class="row-main"><strong>${esc(attention.title)}</strong><small>${esc(run.goal)} · ${relative(attention.created_at)}</small><span class="attention-preview">${esc(attention.prompt)}</span></span><span class="badge awaiting-input">${permission ? 'approval' : 'answer'}</span><span class="chevron">${icon('chevron')}</span></button>`;
}

function runRow(run: Run) {
  return `<button class="run-row" data-run="${esc(run.run_id)}"><span class="run-state ${statusClass(run.status)}"></span><span class="row-main"><strong>${esc(run.goal)}</strong><small>${esc(run.runtime)} · ${relative(run.updated_at)}</small></span><span class="badge ${statusClass(run.status)}">${esc(humanStatus(run.status))}</span><span class="chevron">${icon('chevron')}</span></button>`;
}

function goalCard(goal: Goal, data: Dashboard) {
  const spent = Number(data.goal_usage?.[goal.id] || 0);
  const budget = Number(goal.budget_usd || 20);
  return `<div class="goal-row"><span class="goal-dot ${goal.enabled ? 'on' : ''}"></span><span class="row-main"><strong>${esc(goal.title)}</strong><small>Learn ${esc(goal.cadence)} · review ${esc(goal.review_cadence || 'weekly')} · summary ${esc(goal.summary_cadence || 'monthly')}</small><small>Next learn ${relative(goal.next_run_at)} · review ${relative(goal.next_review_at)} · $${spent.toFixed(2)} / $${budget.toFixed(2)} used this month</small></span><span class="runtime-label">${esc(goal.runtime.replace('-code', ''))}</span><button class="mini-button" data-goal-edit="${esc(goal.id)}">Edit</button><button class="mini-button" data-goal-toggle="${esc(goal.id)}">${goal.enabled ? 'Pause' : 'Resume'}</button><button class="mini-button run-now" data-goal-run="${esc(goal.id)}">Run now</button><button class="mini-button" data-goal-delete="${esc(goal.id)}">Delete</button></div>`;
}

function today(state: ViewState) {
  const { data } = state;
  const active = data.runs.filter((run) => data.active_runs.some((activeRun) => activeRun.run_id === run.run_id));
  const awaiting = data.attentions.map((attention) => ({ attention, run: data.runs.find((run) => run.run_id === attention.run_id) })).filter((item): item is { attention: Attention; run: Run } => Boolean(item.run));
  const upcoming = data.goals.filter((goal) => goal.enabled).slice(0, 3);
  const dateLabel = new Date().toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric' });
  return `<div class="page-heading"><div><div class="eyebrow">${esc(dateLabel)}</div><h1>Good morning, Steven</h1><p class="lede">Your learning system is quiet until it needs your judgment.</p></div><button class="primary" id="new-learning">+ Start learning</button></div>
    <div class="metric-grid"><div class="metric"><span>Needs your input</span><strong>${data.attentions.length}</strong><em class="amber">Runtime questions</em></div><div class="metric"><span>Active runs</span><strong>${active.length}</strong><em class="teal">Background sessions</em></div><div class="metric"><span>Needs review</span><strong>${data.review_count}</strong><em class="amber">Candidate memory</em></div><div class="metric"><span>Scheduled goals</span><strong>${data.goals.filter((goal) => goal.enabled).length}</strong><em class="blue">Automatic cadence</em></div></div>
    <div class="content-grid"><section class="panel attention"><div class="panel-head"><div><h2>Attention queue</h2><p>Only a focused question or approval request interrupts your day.</p></div><span class="panel-count">${awaiting.length}</span></div>${awaiting.length ? awaiting.slice(0, 5).map(({ attention, run }) => attentionCard(attention, run)).join('') : empty('Nothing needs your attention', 'Methodus will keep learning in the background and return with a concrete decision.')}</section><section class="panel schedule"><div class="panel-head"><div><h2>Up next</h2><p>Learning goals on automatic cadence.</p></div><button class="text-button" data-page="goals">Manage</button></div>${upcoming.length ? upcoming.map((goal) => goalCard(goal, data)).join('') : empty('No goals scheduled', 'Create a goal to let Methodus learn on your schedule.')}</section></div>
    <section class="panel activity"><div class="panel-head"><div><h2>Recent runs</h2><p>Background executor sessions remain inspectable and resumable from this app.</p></div></div>${data.runs.slice(0, 6).map(runRow).join('') || empty('No learning runs yet', 'Start a focused investigation when you have a question worth retaining.')}</section>`;
}

function goals(state: ViewState) {
  const { data } = state;
  return `<div class="page-heading"><div><div class="eyebrow">Automatic learning</div><h1>Learning goals</h1><p class="lede">Set direction once. Methodus schedules research, review, and revalidation.</p></div><button class="primary" id="new-goal">+ New goal</button></div><div class="goal-layout"><section class="panel"><div class="panel-head"><div><h2>Active goals</h2><p>${data.goals.filter((goal) => goal.enabled).length} enabled · scheduler checks every 30 seconds</p></div></div>${data.goals.map((goal) => goalCard(goal, data)).join('') || empty('No learning goals', 'A goal includes the question, sources, cadence, runtime, and permission policy.')}</section><section class="panel explainer"><div class="eyebrow">How it works</div><h2>Human direction, runtime execution</h2><p>Each goal launches a bounded native session. The output stays a CandidateSet until you approve, reject, merge, or request another round.</p><div class="flow"><span>Goal</span><i>→</i><span>Runtime</span><i>→</i><span>Attention</span><i>→</i><span>Review</span></div></section></div>`;
}

function filterButton(value: ReviewFilter, label: string, count: number, current: ReviewFilter) {
  return `<button class="${current === value ? 'selected' : ''}" data-review-filter="${value}" aria-pressed="${current === value}">${label} <b>${count}</b></button>`;
}

function review(state: ViewState) {
  const candidates = state.data.nodes.filter((node) => node.status === 'candidate');
  const stale = state.data.nodes.filter((node) => node.status === 'stale');
  const actionable = state.reviewFilter === 'candidate' ? candidates : state.reviewFilter === 'stale' ? stale : [...candidates, ...stale];
  return `<div class="page-heading"><div><div class="eyebrow">Maintainer judgment</div><h1>Review inbox</h1><p class="lede">Candidate knowledge is never exposed to agents until you decide. Stale knowledge waits for source revalidation.</p></div><div class="segmented">${filterButton('all', 'All', candidates.length + stale.length, state.reviewFilter)}${filterButton('candidate', 'Candidate', candidates.length, state.reviewFilter)}${filterButton('stale', 'Stale', stale.length, state.reviewFilter)}</div></div><div class="review-layout"><section class="panel candidate-list">${actionable.length ? actionable.map((node) => `<button class="candidate-row ${state.selectedNode === node.id ? 'selected' : ''}" data-node="${esc(node.id)}"><span class="candidate-type ${node.node_type}">${esc(node.node_type.slice(0, 1).toUpperCase())}</span><span class="row-main"><strong>${esc(node.title)}</strong><small>${esc(node.summary || 'No summary')} · ${esc(node.visibility)}</small></span><span class="badge ${node.status === 'stale' ? 'stale' : 'candidate'}">${node.status === 'stale' ? 'stale' : 'candidate'}</span></button>`).join('') : empty('Review inbox is clear', 'A completed runtime or source check will create an item here.')}</section><section class="panel detail-panel">${state.selectedNode ? nodeDetail(state, state.selectedNode) : `<div class="detail-placeholder"><div class="empty-icon">${icon('review')}</div><h3>Select a review item</h3><p>Compare evidence, inspect relations, and make a durable decision.</p></div>`}</section></div>`;
}

function nodeDetail(state: ViewState, id: string) {
  const node = state.data.nodes.find((item) => item.id === id);
  if (!node) return '';
  const detail = state.nodeDetails[id];
  const isStale = node.status === 'stale';
  const sourceList = detail?.sources?.length ? `<div class="candidate-copy"><h3>Evidence</h3>${detail.sources.map((source) => `<div class="source-evidence"><code>${esc(source.path)}</code><small>${esc(source.fingerprint || 'fingerprint unavailable')}</small></div>`).join('')}</div>` : '';
  const relationList = detail?.edges?.length ? `<div class="candidate-copy"><h3>Relations</h3>${detail.edges.map((edge) => `<div class="relation-row"><span>${esc(edge.relation)}</span><code>${esc(edge.to_id)}</code></div>`).join('')}</div>` : '';
  const revisionList = detail?.revisions?.length ? `<div class="candidate-copy"><h3>Other revisions</h3><div class="revision-grid">${detail.revisions.map((revision) => `<article class="revision-card"><div><span class="badge ${statusClass(revision.status)}">${esc(revision.status || 'candidate')}</span><code>${esc(revision.id)}</code></div><pre class="doc-preview">${esc(revision.content)}</pre></article>`).join('')}</div></div>` : '';
  const runAction = !isStale && detail?.run_id ? `<button class="secondary" data-node-run="${esc(detail.run_id)}">Ask another round</button>` : '';
  const actions = isStale ? `<button class="primary" data-review="revalidate" data-node-id="${esc(id)}">Revalidate source</button>` : `<button class="secondary" data-review="reject" data-node-id="${esc(id)}">Reject</button><button class="secondary" data-review="team" data-node-id="${esc(id)}">Approve to Team</button>${runAction}<button class="secondary" data-merge-node="${esc(id)}">Merge into…</button><button class="primary" data-review="approve" data-node-id="${esc(id)}">Approve Personal</button>`;
  return `<div class="detail-head"><div><span class="eyebrow">${esc(node.node_type)} · ${esc(node.visibility)}</span><h2>${esc(node.title)}</h2><p>${esc(node.summary || '')}</p></div><span class="badge ${isStale ? 'stale' : 'candidate'}">${isStale ? 'stale' : 'candidate'}</span></div><div class="detail-meta"><span>Source</span><code>${esc(node.path)}</code></div>${detail ? `<div class="candidate-copy"><h3>${isStale ? 'Current content' : 'Candidate content'}</h3><pre class="doc-preview">${esc(detail.content)}</pre></div>` : `<div class="candidate-copy"><p>Loading content…</p></div>`}${revisionList}${sourceList}${relationList}<div class="candidate-copy"><h3>${isStale ? 'Revalidation checklist' : 'Review checklist'}</h3><p>${isStale ? 'Check the changed source, confirm the claim still holds, then explicitly revalidate or leave it stale.' : 'Does this conclusion match the evidence? Is it scoped narrowly enough for another agent to apply safely? What would falsify it?'}</p></div><div class="action-row">${actions}</div>`;
}

function library(state: ViewState) {
  const nodes = state.data.nodes.filter((node) => [node.title, node.summary, node.id, ...node.tags].join(' ').toLowerCase().includes(state.query.toLowerCase()) && node.status !== 'candidate' && (state.libraryFilter === 'all' || node.node_type === state.libraryFilter));
  const tab = (value: LibraryFilter, label: string) => `<button class="${state.libraryFilter === value ? 'selected' : ''}" data-library-filter="${value}" aria-pressed="${state.libraryFilter === value}">${label}</button>`;
  return `<div class="page-heading"><div><div class="eyebrow">Reviewed memory</div><h1>Library</h1><p class="lede">Knowledge, Methods, and Experiences available to consumer agents.</p></div><div class="library-tabs">${tab('all', 'All')}${tab('knowledge', 'Knowledge')}${tab('method', 'Methods')}${tab('experience', 'Experiences')}</div></div><div class="library-layout"><section class="panel node-list">${nodes.length ? nodes.map((node) => `<button class="node-row ${state.selectedNode === node.id ? 'selected' : ''}" data-node="${esc(node.id)}"><span class="node-type ${node.node_type}">${node.node_type === 'knowledge' ? 'K' : node.node_type === 'method' ? 'M' : 'E'}</span><span class="row-main"><strong>${esc(node.title)}</strong><small>${esc(node.summary || '')}</small></span><span class="badge ${statusClass(node.status)}">${esc(node.status)}</span></button>`).join('') : empty('No matching memory', 'Try a different search or approve a candidate from Review.')}</section><section class="panel detail-panel">${state.selectedNode ? libraryDetail(state, state.selectedNode) : `<div class="detail-placeholder"><div class="empty-icon">${icon('library')}</div><h3>Browse the graph</h3><p>Search is bounded to reviewed content. Select a node to inspect its sources and relations.</p></div>`}</section></div>`;
}

function libraryDetail(state: ViewState, id: string) {
  const node = state.data.nodes.find((item) => item.id === id);
  if (!node) return '';
  const staleAction = node.status === 'stale' ? `<div class="action-row"><button class="primary" data-review="revalidate" data-node-id="${esc(id)}">Revalidate source</button></div>` : '';
  return `<div class="detail-head"><div><span class="eyebrow">${esc(node.node_type)} · ${esc(node.visibility)}</span><h2>${esc(node.title)}</h2><p>${esc(node.summary || '')}</p></div><span class="badge ${statusClass(node.status)}">${esc(node.status)}</span></div><div class="detail-meta"><span>Path</span><code>${esc(node.path)}</code></div><div class="candidate-copy"><h3>Agent retrieval</h3><p>This node is available through the read-only Methodus connector. The runtime will receive only the relevant facet and bounded context.</p></div>${staleAction}<div class="tag-list">${node.tags.map((tag) => `<span>#${esc(tag)}</span>`).join('')}</div>`;
}

function sources(state: ViewState) {
  const { data } = state;
  const runs = data.runs.filter((run) => run.status === 'awaiting_review');
  const configured = data.goals.flatMap((goal) => (goal.sources || []).map((source) => ({ source, goal: goal.title })));
  const stale = data.nodes.filter((node) => node.status === 'stale');
  return `<div class="page-heading"><div><div class="eyebrow">Evidence surface</div><h1>Sources</h1><p class="lede">Source manifests remain attached to runs so freshness can be reviewed before memory changes.</p></div></div><div class="content-grid"><section class="panel"><div class="panel-head"><div><h2>Registered roots</h2><p>Local roots shared with the native runtime by explicit permission.</p></div></div><div class="source-root"><span class="source-icon">${icon('folder')}</span><span class="row-main"><strong>Launch workspace</strong><small>${esc(data.home)}</small></span><span class="badge committed">available</span></div><div class="source-root"><span class="source-icon">${icon('graph')}</span><span class="row-main"><strong>Team graph</strong><small>${esc(data.team.root)}</small></span><span class="badge ${data.team.dirty ? 'candidate' : 'committed'}">${data.team.dirty ? 'changes' : 'clean'}</span></div>${configured.length ? configured.map(({ source, goal }) => `<div class="source-root"><span class="source-icon">${icon('sources')}</span><span class="row-main"><strong>${esc(source)}</strong><small>Used by ${esc(goal)}</small></span><span class="badge committed">authorized</span></div>`).join('') : '<p class="muted source-empty">No goal-specific roots configured yet.</p>'}</section><section class="panel"><div class="panel-head"><div><h2>Evidence activity</h2><p>${runs.length} run${runs.length === 1 ? '' : 's'} waiting for source review · ${stale.length} stale node${stale.length === 1 ? '' : 's'}.</p></div></div>${stale.length ? stale.slice(0, 4).map((node) => `<button class="run-row" data-node="${esc(node.id)}"><span class="run-state stale"></span><span class="row-main"><strong>${esc(node.title)}</strong><small>Source changed · select to revalidate</small></span><span class="badge stale">stale</span><span class="chevron">${icon('chevron')}</span></button>`).join('') : ''}${runs.map(runRow).join('') || (!stale.length ? empty('No source checks pending', 'A run’s source manifest will appear once the runtime returns.') : '')}</section></div>`;
}

function settings() {
  return `<div class="page-heading"><div><div class="eyebrow">Trust boundary</div><h1>Settings</h1><p class="lede">Methodus is local-first. Runtime sessions stay native; canonical knowledge stays review-gated.</p></div></div><div class="settings-grid"><section class="panel setting-card"><span class="setting-icon">${icon('settings')}</span><h2>Runtime policy</h2><p>Claude Code, Codex, and Cursor can be selected per goal. Plan mode keeps investigation read-only until the runtime asks for approval.</p><button class="text-button" data-settings-message="Runtime and permission policy are configured per learning goal.">Open runtime preferences ${icon('chevron')}</button></section><section class="panel setting-card"><span class="setting-icon">${icon('sources')}</span><h2>Agent connector</h2><p>Consumer agents use one official read-only connector and never write graph files, start runs, or see candidate content.</p><button class="text-button" data-settings-message="Run methodus doctor in the terminal to check connector installation.">Run connector check ${icon('chevron')}</button></section><section class="panel setting-card"><span class="setting-icon">${icon('today')}</span><h2>Notifications</h2><p>Desktop notifications link directly to Today, a Run, or a Review candidate. Quiet hours are respected by the scheduler.</p><button class="text-button" data-settings-message="Notifications are enabled on demand and quiet hours are configured on each Goal.">Configure notifications ${icon('chevron')}</button></section></div>`;
}

function transcriptEvents(details: ViewState['runDetails']) {
  if (!details) return [];
  const visible: typeof details.events = [];
  let lastAssistantText = '';
  for (const event of details.events) {
    if (event.role === 'thinking') continue;
    const normalized = event.text.trim();
    if (event.role === 'assistant') lastAssistantText = normalized;
    if (event.role === 'runtime' && normalized && normalized === lastAssistantText) continue;
    visible.push(event);
  }
  return visible;
}

function runDetail(state: ViewState) {
  const details = state.runDetails;
  if (!details) return empty('Run is loading', 'Methodus is retrieving the durable transcript.');
  const visibleEvents = transcriptEvents(details);
  const attention = details.attention;
  const canContinue = ['awaiting_input', 'failed', 'disconnected'].includes(details.run.status);
  const statusText = attention ? attention.title : details.run.status === 'awaiting_review' ? 'Candidate memory is ready for review' : 'Runtime activity';
  return `<div class="run-workspace"><div class="run-header"><div><div class="eyebrow">${esc(details.run.runtime)} · ${esc(humanStatus(details.run.status))}</div><h1>${esc(details.run.goal)}</h1><p class="lede">${esc(statusText)} · last updated ${relative(details.run.updated_at)}</p></div><span class="badge ${statusClass(details.run.status)}">${esc(humanStatus(details.run.status))}</span></div><div class="run-columns"><section class="run-main-column"><section class="panel run-attention ${attention ? 'open' : ''}"><div class="section-kicker">${attention ? 'Needs your attention' : 'Run summary'}</div>${attention ? `<h2>${esc(attention.prompt)}</h2>${attention.context ? `<p>${esc(attention.context)}</p>` : ''}${attention.tool_name ? `<div class="attention-tool"><span>Requested tool</span><code>${esc(attention.tool_name)}</code></div>` : ''}` : `<h2>${details.run.status === 'awaiting_review' ? 'A candidate set is waiting in Review.' : 'The runtime is working in the background.'}</h2><p>Methodus keeps the transcript and executor session together so you can return to the same investigation without opening a terminal.</p>`}</section><section class="panel transcript"><div class="panel-head"><div><h2>Activity</h2><p>Structured runtime events, with internal reasoning kept out of the primary view.</p></div><span class="event-count">${visibleEvents.length}</span></div><div class="event-stream">${visibleEvents.length ? visibleEvents.map((event) => `<article class="event compact ${esc(event.role)}"><div class="event-meta"><span>${esc(event.role)}</span><time>${relative(event.at)}</time></div><p>${esc(event.text)}</p></article>`).join('') : empty('No events recorded', 'The runtime has not emitted a turn yet.')}</div></section></section><aside class="run-side-column"><section class="panel run-facts"><div class="section-kicker">Run details</div><dl><div><dt>Runtime</dt><dd>${esc(details.run.runtime)}</dd></div><div><dt>Permission</dt><dd>${esc(details.run.permission_mode)}</dd></div><div><dt>Session</dt><dd><code>${esc(details.run.executor_sid || 'negotiating')}</code></dd></div></dl></section>${canContinue ? `<section class="panel follow-up-panel"><div class="section-kicker">Continue in Methodus</div><h2>Give the runtime its next instruction</h2><p>Answer the question or add a correction. The response is recorded and sent to the same executor session.</p><form id="follow-up" class="follow-up"><textarea name="prompt" rows="5" required placeholder="Write the decision, constraint, or follow-up evidence the runtime needs…"></textarea><button class="primary">Send and continue</button></form></section>` : details.run.status === 'awaiting_review' ? `<section class="panel follow-up-panel"><div class="section-kicker">Next step</div><h2>Review the candidate memory</h2><p>Nothing becomes canonical until you make an explicit decision in Review.</p><button class="secondary" data-page="review">Open Review inbox</button></section>` : ''}</aside></div></div>`;
}

export function renderPage(state: ViewState) {
  switch (state.page) {
    case 'today': return today(state);
    case 'goals': return goals(state);
    case 'review': return review(state);
    case 'library': return library(state);
    case 'sources': return sources(state);
    case 'run': return runDetail(state);
    case 'settings': return settings();
  }
}
