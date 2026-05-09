import { json } from '@sveltejs/kit';
import { createBanRepository } from '$lib/server/ban/repository';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = async () => {
  const repository = await createBanRepository();

  return json({
    summary: repository.summary(),
    source: repository.sourceProfile()
  });
};
