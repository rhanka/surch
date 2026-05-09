import { json } from '@sveltejs/kit';
import { compareBanQuery } from '$lib/server/engines';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ fetch }) => {
  return json(await compareBanQuery('match_label', fetch));
};
