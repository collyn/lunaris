import { useId } from 'react';

/** The Lunaris mark without its tile, in one colour (currentColor: white on
 *  the dark theme, black on the light one): the two displays of
 *  public/favicon.svg (same coordinates) with their screens cut out, the
 *  link behind them and the hub as a ring. Gaps keep the shapes apart now
 *  that they share a colour. */
export function BrandMark({ className, size = 30 }: { className?: string; size?: number }) {
  // Unique mask ids: more than one mark may be on the page.
  const id = useId().replace(/:/g, '');
  const m = (name: string) => `${name}-${id}`;
  const height = size;
  const width = Math.round((size * 784) / 556);
  // Shapes the masks cut out: the front display grown by the gap, the hub's clearance.
  const frontGap = <rect x="144" y="400" width="480" height="332" rx="84" fill="black" />;
  const rearGap = <rect x="509" y="218" width="451" height="336" rx="84" fill="black" />;
  const hubGap = <circle cx="572" cy="478" r="88" fill="black" />;
  return (
    <svg className={className} viewBox="160 230 784 556" width={width} height={height} aria-hidden="true" focusable="false">
      <defs>
        <mask id={m('arc')} maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
          <rect x="0" y="0" width="1024" height="1024" fill="white" />
          {frontGap}
          {rearGap}
          {hubGap}
        </mask>
        <mask id={m('rear')} maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
          <rect x="0" y="0" width="1024" height="1024" fill="white" />
          <rect x="569" y="279" width="331" height="213" rx="29" fill="black" />
          {frontGap}
          {hubGap}
        </mask>
        <mask id={m('front')} maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
          <rect x="0" y="0" width="1024" height="1024" fill="white" />
          <rect x="204" y="461" width="360" height="209" rx="29" fill="black" />
          {hubGap}
        </mask>
      </defs>
      <g fill="currentColor">
        {/* connection arc, behind the displays */}
        <path d="M440 405 C500 272 682 280 740 390 C814 538 705 659 590 640" fill="none" stroke="currentColor"
          strokeWidth="39" strokeLinecap="round" mask={`url(#${m('arc')})`} />
        {/* rear display (its stand, hidden behind the front one in the
            coloured icon, would run into the link here) */}
        <g mask={`url(#${m('rear')})`}>
          <rect x="533" y="242" width="403" height="288" rx="62" />
        </g>
        {/* front display, its stand and foot */}
        <g mask={`url(#${m('front')})`}>
          <rect x="168" y="424" width="432" height="284" rx="62" />
          <path d="M345 708 v35 h85 v-35" />
          <rect x="286" y="740" width="205" height="36" rx="18" />
        </g>
        {/* connection hub: a ring and its centre */}
        <circle cx="572" cy="478" r="52" fill="none" stroke="currentColor" strokeWidth="26" />
        <circle cx="572" cy="478" r="20" />
      </g>
    </svg>
  );
}
