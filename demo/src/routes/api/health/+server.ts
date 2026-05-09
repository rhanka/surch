import { json } from '@sveltejs/kit';
import { createBanRepository } from '$lib/server/ban/repository';
import type { RequestHandler } from './$types';

export const GET = (async () => {
  const repository = await createBanRepository();
  const summary = repository.summary();
  const source = repository.sourceProfile();

  return json({
    status: 'ok',
    dataset: summary.name,
    documentCount: summary.documentCount,
    source: {
      kind: source.kind
    }
  });
}) satisfies RequestHandler;
