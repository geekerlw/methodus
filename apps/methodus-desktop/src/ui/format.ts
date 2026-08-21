export const esc = (value: unknown) => String(value ?? '').replace(/[&<>"']/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;' }[char]!));

export const relative = (value?: string) => value
  ? new Date(value).toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
  : '—';

export const statusClass = (status?: string) => (status ?? '').replaceAll('_', '-');

export const humanStatus = (status?: string) => (status ?? 'unknown').replaceAll('_', ' ');
