const paths: Record<string, string> = {
  today: '<path d="M4 5.5h16M6.5 3.5v4M17.5 3.5v4M5 8.5h14v10H5z"/><path d="M8 12h2M12 12h2M16 12h1M8 15.5h2M12 15.5h2"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  goals: '<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="4.5"/><circle cx="12" cy="12" r="1" fill="currentColor" stroke="none"/>',
  review: '<path d="M5 4.5h14v15H5z"/><path d="m8 12 2.2 2.2L16 8.5"/>',
  library: '<path d="M5 5.5A2.5 2.5 0 0 1 7.5 3H19v16H7.5A2.5 2.5 0 0 0 5 21z"/><path d="M5 5.5v15M9 7h6M9 10h6"/>',
  sources: '<path d="M9.5 14.5 14 10m-6.2 7.8-1.1 1.1a3 3 0 0 1-4.2-4.2l3.2-3.2a3 3 0 0 1 4.2 0m.6-4.6 1.1-1.1a3 3 0 0 1 4.2 4.2l-3.2 3.2a3 3 0 0 1-4.2 0"/>',
  settings: '<path d="M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z"/><path d="m19 13.5 1.4 1.1-1.6 2.8-1.7-.6a7.4 7.4 0 0 1-1.5.9l-.2 1.8h-3.2l-.3-1.8a7.4 7.4 0 0 1-1.5-.9l-1.7.6-1.6-2.8L6.5 13.5a7.4 7.4 0 0 1 0-1.8l-1.4-1.1 1.6-2.8 1.7.6a7.4 7.4 0 0 1 1.5-.9l.2-1.8h3.2l.3 1.8a7.4 7.4 0 0 1 1.5.9l1.7-.6 1.6 2.8-1.4 1.1a7.4 7.4 0 0 1 0 1.8Z"/>',
  refresh: '<path d="M20 11a8 8 0 0 0-14.7-3L4 10m0-4v4h4M4 13a8 8 0 0 0 14.7 3L20 14m0 4v-4h-4"/>',
  close: '<path d="m6 6 12 12M18 6 6 18"/>',
  chevron: '<path d="m9 5 7 7-7 7"/>',
  alert: '<path d="M12 4 21 20H3z"/><path d="M12 9v5M12 17.5v.1"/>',
  pulse: '<path d="M3 12h4l2-5 4 10 2-5h4"/>',
  spark: '<path d="m12 3 1.5 6.5L20 11l-6.5 1.5L12 19l-1.5-6.5L4 11l6.5-1.5z"/>',
  methodus: '<path d="M6 17V8.5A3.5 3.5 0 0 1 9.5 5h5A3.5 3.5 0 0 1 18 8.5V17"/><path d="m6 16 6-7 6 7"/><path d="M8.5 18h7"/>',
  folder: '<path d="M3.5 7.5h6l1.8 2h9.2v8.8a1.7 1.7 0 0 1-1.7 1.7H5.2a1.7 1.7 0 0 1-1.7-1.7z"/><path d="M3.5 7.5V6a1.5 1.5 0 0 1 1.5-1.5h4l1.8 2h7.7A1.5 1.5 0 0 1 20 8v1.5"/>',
  graph: '<circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="m8.2 11 7.5-4M8.2 13l7.5 4"/>',
};

export function icon(name: string) {
  return `<svg class="ui-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name] ?? paths.spark}</svg>`;
}
