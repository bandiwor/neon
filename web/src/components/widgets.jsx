import { useEffect, useRef, useState } from 'preact/hooks';
import { Icon } from './icons.jsx';

/** A button with an icon and/or a label; `title` doubles as the accessible name. */
export function Button({ icon, children, primary, danger, active, title, onClick, disabled, class: cls = '', ...rest }) {
  return (
    <button
      type="button"
      class={`btn ${primary ? 'btn-primary' : ''} ${danger ? 'btn-danger' : ''} ${active ? 'is-active' : ''} ${children ? '' : 'btn-icon'} ${cls}`}
      title={title}
      aria-label={children ? undefined : title}
      aria-pressed={active === undefined ? undefined : !!active}
      onClick={onClick}
      disabled={disabled}
      {...rest}
    >
      {icon && <Icon name={icon} />}
      {children && <span>{children}</span>}
    </button>
  );
}

/** A dropdown that opens under its button and closes on outside click or Escape. */
export function Menu({ label, icon, title, children, align = 'left' }) {
  const [open, setOpen] = useState(false);
  const ref = useRef();
  useEffect(() => {
    if (!open) return;
    const close = (e) => {
      if (!ref.current?.contains(e.target)) setOpen(false);
    };
    const key = (e) => e.key === 'Escape' && setOpen(false);
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', key);
    };
  }, [open]);
  return (
    <div class="menu" ref={ref}>
      <Button icon={icon} title={title} onClick={() => setOpen(!open)} active={open}>
        {label}
      </Button>
      {open && (
        <div class={`menu-pop align-${align}`} role="menu" onClick={() => setOpen(false)}>
          {children}
        </div>
      )}
    </div>
  );
}

export function MenuItem({ icon, children, hint, onClick, disabled }) {
  return (
    <button type="button" role="menuitem" class="menu-item" onClick={onClick} disabled={disabled}>
      <span class="menu-item-icon">{icon && <Icon name={icon} />}</span>
      <span class="menu-item-label">{children}</span>
      {hint && <kbd>{hint}</kbd>}
    </button>
  );
}

export function MenuSeparator() {
  return <div class="menu-sep" role="separator" />;
}

/** A modal dialog: Escape and the backdrop close it. */
export function Dialog({ title, onClose, children, wide, footer }) {
  const ref = useRef();
  useEffect(() => {
    const prev = document.activeElement;
    ref.current?.querySelector('input, textarea, button')?.focus();
    const key = (e) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('keydown', key);
      prev?.focus?.();
    };
  }, []);
  return (
    <div class="backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div class={`dialog ${wide ? 'dialog-wide' : ''}`} role="dialog" aria-modal="true" aria-label={title} ref={ref}>
        <header class="dialog-head">
          <h2>{title}</h2>
          <Button icon="x" title="×" onClick={onClose} />
        </header>
        <div class="dialog-body">{children}</div>
        {footer && <footer class="dialog-foot">{footer}</footer>}
      </div>
    </div>
  );
}

export function Field({ label, hint, children }) {
  return (
    <label class="field">
      <span class="field-label">{label}</span>
      {children}
      {hint && <span class="field-hint">{hint}</span>}
    </label>
  );
}

/** A group of mutually exclusive choices drawn as a segmented control. */
export function Segmented({ value, options, onChange, label }) {
  return (
    <div class="segmented" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          type="button"
          role="radio"
          aria-checked={value === o.value}
          class={value === o.value ? 'is-active' : ''}
          onClick={() => onChange(o.value)}
          title={o.title}
        >
          {o.icon && <Icon name={o.icon} />}
          {o.label && <span>{o.label}</span>}
        </button>
      ))}
    </div>
  );
}

export function Switch({ checked, onChange, children }) {
  return (
    <label class="switch">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.currentTarget.checked)} />
      <span class="switch-box" aria-hidden="true" />
      <span>{children}</span>
    </label>
  );
}

/** A drag handle that reports how far it has moved; `axis` is 'x' or 'y'. */
export function Resizer({ axis, onDrag, onDone }) {
  const start = (e) => {
    e.preventDefault();
    const origin = axis === 'x' ? e.clientX : e.clientY;
    const move = (m) => onDrag((axis === 'x' ? m.clientX : m.clientY) - origin);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('is-resizing');
      onDone?.();
    };
    document.body.classList.add('is-resizing');
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return <div class={`resizer resizer-${axis}`} onPointerDown={start} role="separator" aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'} />;
}

/** An input that takes the focus (and selects its text) when it appears. */
export function FocusInput(props) {
  const ref = useRef();
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select?.();
  }, []);
  return <input ref={ref} {...props} />;
}
