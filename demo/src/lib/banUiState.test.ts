import { describe, expect, it } from 'vitest';
import type { BanDocument } from './types';
import {
  banDatasetUiState,
  canCompareAddress,
  compareAddressError,
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

  it('blocks comparison until a BAN address has been selected', () => {
    expect(canCompareAddress(null)).toBe(false);
    expect(compareAddressError(null)).toBe('Sélectionne une adresse avant de comparer.');

    expect(canCompareAddress(officialDocument)).toBe(true);
    expect(compareAddressError(officialDocument)).toBeNull();
  });

  it('marks ban_tiny as the local visual fallback instead of the active BAN dataset', () => {
    expect(
      banDatasetUiState({
        summary: {
          name: 'ban_tiny',
          documentCount: 3
        },
        source: {
          kind: 'tiny',
          offline: true,
          downloadCommand: 'npm run ban:download'
        }
      })
    ).toEqual({
      documentCount: 3,
      isActiveDataset: false,
      name: 'ban_tiny',
      sourceLabel: 'Fallback local: ban_tiny',
      status: 'ban_tiny: 3 adresse(s) prêtes',
      usesTinyFallback: true
    });
  });

  it('marks a loaded BAN CSV as the active dataset', () => {
    expect(
      banDatasetUiState({
        summary: {
          name: 'adresses-75.csv.gz',
          documentCount: 25000
        },
        source: {
          kind: 'csv',
          path: 'data/ban/adresses-75.csv.gz'
        }
      })
    ).toEqual({
      documentCount: 25000,
      isActiveDataset: true,
      name: 'adresses-75.csv.gz',
      sourceLabel: 'Dataset actif: adresses-75.csv.gz',
      status: 'adresses-75.csv.gz: 25000 adresse(s) prêtes',
      usesTinyFallback: false
    });
  });
});
