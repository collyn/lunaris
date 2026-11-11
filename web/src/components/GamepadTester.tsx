import { useEffect, useState } from 'react';
import { B, connectedPads, gamepadUnavailableReason, vibrate, type PadKind, type PadState } from '../gamepad';
import { t } from '../i18n';

// Live view of this browser's controllers: every button, stick and trigger
// lights up as it is used, labelled the way the controller is (PlayStation,
// Xbox, Nintendo), with a vibration test.

type Labels = { face: [string, string, string, string]; faceColors: [string, string, string, string]; lb: string; rb: string; lt: string; rt: string; back: string; start: string; guide: string; ls: string; rs: string; touchpad?: string };

// face order: bottom (A), right (B), left (X), top (Y) — positions, as the
// standard mapping numbers them.
const LABELS: Record<PadKind, Labels> = {
  playstation: { face: ['✕', '○', '□', '△'], faceColors: ['#5b8def', '#e5534b', '#d36fb4', '#3fb68b'], lb: 'L1', rb: 'R1', lt: 'L2', rt: 'R2', back: 'Create', start: 'Options', guide: 'PS', ls: 'L3', rs: 'R3', touchpad: 'Touchpad' },
  xbox: { face: ['A', 'B', 'X', 'Y'], faceColors: ['#3fb68b', '#e5534b', '#5b8def', '#e3b341'], lb: 'LB', rb: 'RB', lt: 'LT', rt: 'RT', back: 'View', start: 'Menu', guide: 'Xbox', ls: 'LS', rs: 'RS' },
  nintendo: { face: ['B', 'A', 'Y', 'X'], faceColors: ['#9aa4b2', '#9aa4b2', '#9aa4b2', '#9aa4b2'], lb: 'L', rb: 'R', lt: 'ZL', rt: 'ZR', back: '−', start: '+', guide: 'Home', ls: 'LS', rs: 'RS', touchpad: 'Capture' },
  generic: { face: ['A', 'B', 'X', 'Y'], faceColors: ['#3fb68b', '#e5534b', '#5b8def', '#e3b341'], lb: 'LB', rb: 'RB', lt: 'LT', rt: 'RT', back: 'Back', start: 'Start', guide: 'Home', ls: 'LS', rs: 'RS' },
};

const KIND_NAME: Record<PadKind, string> = { playstation: 'PlayStation', xbox: 'Xbox', nintendo: 'Nintendo', generic: 'Generic' };

function usePads(): PadState[] {
  const [pads, setPads] = useState<PadState[]>(() => connectedPads());
  useEffect(() => {
    let raf = 0;
    let last = '';
    const tick = () => {
      const now = connectedPads();
      const key = JSON.stringify(now);
      if (key !== last) { last = key; setPads(now); }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    const wake = () => setPads(connectedPads());
    window.addEventListener('gamepadconnected', wake);
    window.addEventListener('gamepaddisconnected', wake);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener('gamepadconnected', wake);
      window.removeEventListener('gamepaddisconnected', wake);
    };
  }, []);
  return pads;
}

const ON = 'var(--accent)';

function Diagram({ pad }: { pad: PadState }) {
  const L = LABELS[pad.kind];
  const on = (bit: number) => (pad.buttons & bit) !== 0;
  const off = 'rgb(var(--ink-rgb) / 0.08)';
  const stroke = 'rgb(var(--ink-rgb) / 0.35)';
  const text = 'var(--text-strong)';
  const stick = (cx: number, cy: number, x: number, y: number, pressed: boolean, label: string) => (
    <g>
      <circle cx={cx} cy={cy} r={30} fill={pressed ? ON : off} stroke={stroke} strokeWidth={1.5} />
      <circle cx={cx + (x / 32767) * 18} cy={cy + (y / 32767) * 18} r={12} fill="var(--surface)" stroke={stroke} strokeWidth={1.5} />
      <text x={cx} y={cy + 46} textAnchor="middle" fontSize="11" fill="var(--text-muted)">{label}</text>
    </g>
  );
  const trigger = (x: number, value: number, label: string) => (
    <g>
      <rect x={x} y={8} width={70} height={14} rx={7} fill={off} stroke={stroke} />
      <rect x={x} y={8} width={(70 * value) / 255} height={14} rx={7} fill={ON} />
      <text x={x + 35} y={19} textAnchor="middle" fontSize="10" fontWeight="600" fill={text}>{label} {Math.round((value * 100) / 255)}%</text>
    </g>
  );
  const bumper = (x: number, pressed: boolean, label: string) => (
    <g>
      <rect x={x} y={28} width={70} height={16} rx={6} fill={pressed ? ON : off} stroke={stroke} />
      <text x={x + 35} y={40} textAnchor="middle" fontSize="10" fontWeight="600" fill={pressed ? '#fff' : text}>{label}</text>
    </g>
  );
  const face = (cx: number, cy: number, bit: number, i: number) => (
    <g>
      <circle cx={cx} cy={cy} r={14} fill={on(bit) ? L.faceColors[i] : off} stroke={L.faceColors[i]} strokeWidth={2} />
      <text x={cx} y={cy + 4.5} textAnchor="middle" fontSize="13" fontWeight="700" fill={on(bit) ? '#fff' : L.faceColors[i]}>{L.face[i]}</text>
    </g>
  );
  const dpad = (x: number, y: number, w: number, h: number, bit: number) => (
    <rect x={x} y={y} width={w} height={h} rx={3} fill={on(bit) ? ON : off} stroke={stroke} />
  );
  const small = (cx: number, cy: number, bit: number, label: string) => (
    <g>
      <rect x={cx - 22} y={cy - 9} width={44} height={18} rx={9} fill={on(bit) ? ON : off} stroke={stroke} />
      <text x={cx} y={cy + 3.5} textAnchor="middle" fontSize="9" fontWeight="600" fill={on(bit) ? '#fff' : text}>{label}</text>
    </g>
  );
  return (
    <svg viewBox="0 0 400 250" className="pad-diagram" role="img" aria-label={pad.name}>
      <path d="M70 52 Q200 40 330 52 Q385 60 392 150 Q398 225 350 232 Q318 236 290 200 L110 200 Q82 236 50 232 Q2 225 8 150 Q15 60 70 52 Z" fill="rgb(var(--ink-rgb) / 0.03)" stroke={stroke} strokeWidth={1.5} />
      {trigger(55, pad.lt, L.lt)}
      {trigger(275, pad.rt, L.rt)}
      {bumper(55, on(B.LB), L.lb)}
      {bumper(275, on(B.RB), L.rb)}
      {L.touchpad && (
        <g>
          <rect x={160} y={58} width={80} height={34} rx={6} fill={on(B.TOUCHPAD) ? ON : off} stroke={stroke} />
          <text x={200} y={79} textAnchor="middle" fontSize="9" fill={on(B.TOUCHPAD) ? '#fff' : 'var(--text-muted)'}>{L.touchpad}</text>
        </g>
      )}
      {small(L.touchpad ? 135 : 165, 75, B.BACK, L.back)}
      {small(L.touchpad ? 265 : 235, 75, B.START, L.start)}
      <circle cx={200} cy={L.touchpad ? 112 : 92} r={13} fill={on(B.GUIDE) ? ON : off} stroke={stroke} />
      <text x={200} y={(L.touchpad ? 112 : 92) + 3.5} textAnchor="middle" fontSize="8" fontWeight="700" fill={on(B.GUIDE) ? '#fff' : text}>{L.guide}</text>
      {stick(85, 105, pad.lx, pad.ly, on(B.LS), L.ls)}
      {dpad(129, 136, 22, 24, B.UP)}
      {dpad(129, 182, 22, 24, B.DOWN)}
      {dpad(105, 160, 24, 22, B.LEFT)}
      {dpad(151, 160, 24, 22, B.RIGHT)}
      {face(315, 135, B.A, 0)}
      {face(345, 105, B.B, 1)}
      {face(285, 105, B.X, 2)}
      {face(315, 75, B.Y, 3)}
      {stick(250, 165, pad.rx, pad.ry, on(B.RS), L.rs)}
    </svg>
  );
}

export default function GamepadTester() {
  const pads = usePads();
  const reason = gamepadUnavailableReason();
  if (reason) return <div className="pad-empty">{t(reason)}</div>;
  if (!pads.length) {
    return <div className="pad-empty">{t('No controller found. Connect one (USB or Bluetooth) and press any button on it.')}</div>;
  }
  return (
    <div className="pad-list">
      {pads.map(pad => (
        <div key={pad.index} className="pad-card">
          <div className="pad-head">
            <div>
              <div className="pad-name">{pad.name}</div>
              <div className="pad-meta">
                {t('Controller %1', pad.index + 1)} · {KIND_NAME[pad.kind]}
                {!pad.standard && ' · ' + t('unknown layout: buttons may be in other places')}
              </div>
            </div>
            <button type="button" className="btn-secondary" disabled={!pad.rumble} onClick={() => void vibrate(pad.index)} title={pad.rumble ? '' : t('This browser cannot make this controller vibrate')}>
              {t('Test vibration')}
            </button>
          </div>
          <Diagram pad={pad} />
          <div className="pad-axes">
            <span>{t('Left stick')} {(pad.lx / 32767).toFixed(2)}, {(pad.ly / 32767).toFixed(2)}</span>
            <span>{t('Right stick')} {(pad.rx / 32767).toFixed(2)}, {(pad.ry / 32767).toFixed(2)}</span>
          </div>
        </div>
      ))}
    </div>
  );
}
