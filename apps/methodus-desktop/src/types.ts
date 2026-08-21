export type Node = {
  id: string;
  node_type: string;
  title: string;
  path: string;
  status?: string;
  summary?: string;
  visibility: string;
  tags: string[];
};

export type Run = {
  run_id: string;
  goal: string;
  runtime: string;
  permission_mode: string;
  status: string;
  executor_sid?: string;
  updated_at: string;
};

export type Goal = {
  id: string;
  title: string;
  prompt: string;
  sources?: string[];
  runtime: string;
  permission_mode: string;
  cadence: string;
  review_cadence?: string;
  summary_cadence?: string;
  source_check_cadence?: string;
  quiet_hours_start?: string;
  quiet_hours_end?: string;
  budget_usd?: number;
  review_policy?: string;
  enabled: boolean;
  next_run_at?: string;
  next_review_at?: string;
  next_summary_at?: string;
  next_source_check_at?: string;
  created_at: string;
  updated_at: string;
};

export type Attention = {
  id: string;
  run_id: string;
  kind: string;
  title: string;
  prompt: string;
  context?: string;
  tool_name?: string;
  tool_input?: string;
  status: string;
  created_at: string;
  resolved_at?: string;
  response?: string;
};

export type Dashboard = {
  home: string;
  nodes: Node[];
  runs: Run[];
  goals: Goal[];
  active_runs: { run_id: string; runtime: string; goal_id?: string }[];
  team: { id: string; root: string; is_git: boolean; branch?: string; dirty: boolean; changes: string[] };
  review_count: number;
  stale_count: number;
  goal_usage?: Record<string, number>;
  attentions: Attention[];
};

export type RunDetails = {
  run: Run;
  events: { at: string; role: string; text: string }[];
  attention?: Attention;
};

export type NodeDetails = {
  node: Node;
  edges: { id: string; from_id: string; relation: string; to_id: string }[];
  kind?: string;
  content: string;
  sources: { path: string; repository?: string; fingerprint?: string }[];
  run_id?: string;
  revisions?: { id: string; path: string; status?: string; content: string }[];
};

export type Page = 'today' | 'goals' | 'review' | 'library' | 'sources' | 'settings' | 'run';
export type ReviewFilter = 'all' | 'candidate' | 'stale';
export type LibraryFilter = 'all' | 'knowledge' | 'method' | 'experience';

export type ViewState = {
  data: Dashboard;
  page: Page;
  query: string;
  selectedNode: string | null;
  selectedRun: string | null;
  reviewFilter: ReviewFilter;
  libraryFilter: LibraryFilter;
  runDetails: RunDetails | null;
  nodeDetails: Record<string, NodeDetails>;
};
