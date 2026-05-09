import { json } from '@sveltejs/kit';
import { createBanRepository } from '$lib/server/ban/repository';
import { DEFAULT_BAN_SUGGESTIONS, MAX_BAN_SUGGESTIONS } from '$lib/server/ban/schema';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request }) => {
  const body = await request.json().catch(() => ({}));
  const query = typeof body.query === 'string' ? body.query : '';
  const limit = clampLimit(body.limit);
  const repository = await createBanRepository();
  const suggestions = repository.suggest({ query, limit });

  return json({
    query,
    limit,
    suggestions
  });
};

function clampLimit(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    return DEFAULT_BAN_SUGGESTIONS;
  }

  return Math.min(Math.trunc(value), MAX_BAN_SUGGESTIONS);
}
