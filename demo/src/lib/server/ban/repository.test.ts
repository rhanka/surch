import { describe, expect, it } from 'vitest';
import type { BanDocument } from '$lib/types';
import { MAX_BAN_SUGGESTIONS } from './schema';
import { createBanRepository, createInMemoryBanRepository } from './repository';

function makeDocument(index: number): BanDocument {
  return {
    city_code: '75101',
    city_name: 'Paris',
    house_number: String(index + 1),
    id: `75101_0001_${String(index + 1).padStart(5, '0')}`,
    label: `${index + 1} Rue de Rivoli 75001 Paris`,
    location: {
      lat: 48.86 + index / 10_000,
      lon: 2.33 + index / 10_000
    },
    postcode: '75001',
    source: 'BAN',
    street_name: 'Rue de Rivoli'
  };
}

describe('BAN repository', () => {
  it('uses ban_tiny as the default offline dataset', async () => {
    const repository = await createBanRepository({});

    expect(repository.summary()).toMatchObject({
      name: 'ban_tiny',
      documentCount: 3
    });
    expect(repository.sourceProfile()).toMatchObject({
      kind: 'tiny',
      offline: true
    });
  });

  it('suggests Rue de Rivoli for a contains query on the street label', async () => {
    const repository = await createBanRepository({});
    const suggestions = repository.suggest({ query: 'rivo' });

    expect(suggestions[0]).toMatchObject({
      id: '75101_0001_00001',
      label: '1 Rue de Rivoli 75001 Paris 1er Arrondissement',
      street_name: 'Rue de Rivoli'
    });
  });

  it('rejects an invalid BAN_CSV_PATH before falling back to tiny data', async () => {
    await expect(
      createBanRepository({ BAN_CSV_PATH: '/tmp/surch-ban-missing.csv' })
    ).rejects.toThrow(/BAN_CSV_PATH/);
  });

  it('caps suggestions at the backend maximum', () => {
    const repository = createInMemoryBanRepository({
      documents: Array.from({ length: MAX_BAN_SUGGESTIONS + 5 }, (_, index) => makeDocument(index)),
      source: {
        kind: 'tiny',
        name: 'ban_tiny',
        offline: true,
        bounded: false
      }
    });

    expect(repository.suggest({ query: 'rivoli', limit: 999 })).toHaveLength(MAX_BAN_SUGGESTIONS);
  });
});
