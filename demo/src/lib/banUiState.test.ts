import { describe, expect, it } from 'vitest';
import type { BanDocument } from './types';
import {
  datasetStatus,
  extractSuggestionDocuments,
  shouldRequestSuggestions
} from './banUiState';

const officialDocument: BanDocument = {
  city_code: '75101',
  city_name: 'Paris 1er Arrondissement',
  house_number: '41',
  id: '75101_8238_00041',
  label: '41 Rue de Rivoli 75001 Paris 1er Arrondissement',
  location: { lat: 48.859, lon: 2.342 },
  postcode: '75001',
  source: 'BAN',
  street_name: 'Rue de Rivoli'
};

describe('BAN demo UI state', () => {
  it('keeps the suggestion list empty until the query can hit the API', () => {
    expect(shouldRequestSuggestions('')).toBe(false);
    expect(shouldRequestSuggestions('r')).toBe(false);
    expect(extractSuggestionDocuments({})).toEqual([]);
  });

  it('uses API suggestions without falling back to the tiny fixture', () => {
    expect(shouldRequestSuggestions('ri')).toBe(true);
    expect(extractSuggestionDocuments({ suggestions: [officialDocument] })).toEqual([
      officialDocument
    ]);
    expect(extractSuggestionDocuments({ suggestions: [] })).toEqual([]);
  });

  it('formats the active dataset summary returned by the API', () => {
    expect(
      datasetStatus({
        summary: {
          name: 'adresses-75.csv.gz',
          documentCount: 25000
        }
      })
    ).toBe('adresses-75.csv.gz: 25000 adresse(s) prêtes');
  });
});
