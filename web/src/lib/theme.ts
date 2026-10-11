import { useCallback, useEffect, useState } from 'react';
import { readStorage, writeStorage } from './storage';

/**
 * Theme handling. The resolved theme lives on <html data-theme="light|dark">
 * only when the user picked one explicitly; with "system" the attribute is
 * removed and `prefers-color-scheme` decides (see styles.css).
 */
export type ThemePreference = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

const KEY = 'flexma:theme';
const EVENT = 'flexma:themechange';

export function getThemePreference(): ThemePreference {
  const v = readStorage(KEY);
  return v === 'light' || v === 'dark' ? v : 'system';
}

export function resolveTheme(pref: ThemePreference = getThemePreference()): ResolvedTheme {
  if (pref !== 'system') return pref;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

/** Apply the stored preference to <html>. Call once before first render. */
export function applyTheme(pref: ThemePreference = getThemePreference()): void {
  const root = document.documentElement;
  if (pref === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', pref);
  root.style.colorScheme = resolveTheme(pref);
}

export function setThemePreference(pref: ThemePreference): void {
  writeStorage(KEY, pref === 'system' ? null : pref);
  applyTheme(pref);
  window.dispatchEvent(new Event(EVENT));
}

/** Current preference + resolved theme, re-rendering on changes (incl. OS changes). */
export function useTheme(): {
  preference: ThemePreference;
  theme: ResolvedTheme;
  setPreference: (pref: ThemePreference) => void;
} {
  const [preference, setPref] = useState<ThemePreference>(getThemePreference);
  const [theme, setTheme] = useState<ResolvedTheme>(() => resolveTheme());

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const sync = () => {
      const pref = getThemePreference();
      applyTheme(pref);
      setPref(pref);
      setTheme(resolveTheme(pref));
    };
    media.addEventListener('change', sync);
    window.addEventListener(EVENT, sync);
    window.addEventListener('storage', sync);
    return () => {
      media.removeEventListener('change', sync);
      window.removeEventListener(EVENT, sync);
      window.removeEventListener('storage', sync);
    };
  }, []);

  const setPreference = useCallback((pref: ThemePreference) => setThemePreference(pref), []);
  return { preference, theme, setPreference };
}
