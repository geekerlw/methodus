import { invoke } from '@tauri-apps/api/core';
import type { Dashboard, Goal, NodeDetails, RunDetails } from './types';

type GoalInput = {
  title: FormDataEntryValue | null;
  prompt: FormDataEntryValue | null;
  sources: string[];
  runtime: FormDataEntryValue | null;
  permissionMode: FormDataEntryValue | null;
  cadence: FormDataEntryValue | null;
  reviewCadence: FormDataEntryValue | null;
  summaryCadence: FormDataEntryValue | null;
  sourceCheckCadence: FormDataEntryValue | null;
  quietHoursStart: string | null;
  quietHoursEnd: string | null;
  budgetUsd: number;
  reviewPolicy: FormDataEntryValue | null;
  enabled: boolean;
};

function command<T>(name: string, args?: Record<string, unknown>) {
  return invoke<T>(name, args);
}

export const api = {
  dashboard: () => command<Dashboard>('get_dashboard'),
  run: (runId: string) => command<RunDetails>('get_run', { runId }),
  node: (nodeId: string) => command<NodeDetails>('get_node', { nodeId }),
  startLearning: (goal: FormDataEntryValue | null, runtime: FormDataEntryValue | null, permissionMode: FormDataEntryValue | null) => command('start_learning', { goal, runtime, permissionMode }),
  saveGoal: (input: GoalInput) => command<Goal>('save_goal', { input }),
  updateGoal: (goalId: string, input: GoalInput) => command<Goal>('update_goal', { goalId, input }),
  deleteGoal: (goalId: string) => command('delete_goal', { goalId }),
  setGoalEnabled: (goalId: string, enabled: boolean) => command('set_goal_enabled', { goalId, enabled }),
  runGoal: (goalId: string) => command('run_goal', { goalId }),
  continueLearning: (runId: string, prompt: string, attentionId?: string | null) => command('continue_learning', { runId, prompt, attentionId: attentionId ?? null }),
  reviewCandidate: (nodeId: string, action: string, targetId: string | null, rationale: string | null) => command<Dashboard>('review_candidate', { nodeId, action, targetId, rationale }),
};
