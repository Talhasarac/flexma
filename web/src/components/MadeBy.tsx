/** "Made by silly toast studio" credit (the studio behind Flexma). */
export function MadeBy({ compact = false }: { compact?: boolean }) {
  return (
    <a className={`made-by${compact ? ' made-by-compact' : ''}`} href="https://sillytoast.studio" target="_blank" rel="noreferrer" title="Made by Silly Toast Studio">
      <img src="/sillytoast.svg" alt="" width={compact ? 14 : 18} height={compact ? 14 : 18} />
      <span>
        {compact ? 'by ' : 'Made by '}
        <b>silly toast</b>
        {!compact && <small>studio</small>}
      </span>
    </a>
  );
}
