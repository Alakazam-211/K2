// Settings → Providers / Models: a harness's "Default model" (0.46.13,
// settings key `defaultModels`). Every new chat K2 starts with that harness
// on this server begins on it; unset = "Harness default" (the CLI picks).
// The same picker as the Thread's `/model`, minus its "Server default" row.

import { useCallback, useEffect, useRef, useState, type JSX } from 'react'
import { ModelPicker, MODEL_PICKER_WORDS } from '@/components/Thread/ModelPicker'
import { binaryTokenFromCommand, defaultModelHarness, modelLabel, modelPickerRows } from '@/lib/model-choices'
import { useDefaultModelsStore } from '@/stores/default-models'
import { SETTING_ROW_LABEL } from '../controls/SettingControls'

/** The harness a preset command can have a server default for, if any. */
export function presetModelHarness(command: string): string | null {
  return defaultModelHarness(binaryTokenFromCommand(command))
}

export function DefaultModelField({ harness }: { harness: string }): JSX.Element {
  const models = useDefaultModelsStore((s) => s.models)
  const loaded = useDefaultModelsStore((s) => s.loaded)
  const error = useDefaultModelsStore((s) => s.error)
  const load = useDefaultModelsStore((s) => s.load)
  const setDefault = useDefaultModelsStore((s) => s.setDefault)
  const [open, setOpen] = useState(false)
  const btn = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (!loaded) void load()
  }, [loaded, load])
  const value = models[harness] ?? ''
  const rows = useCallback(
    (query: string) => modelPickerRows({ harness, mode: 'setting', current: value, query }),
    [harness, value],
  )
  return (
    <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5" data-testid={`default-model-${harness}`}>
      <span className={SETTING_ROW_LABEL}>Default model</span>
      <button
        ref={btn}
        type="button"
        data-k2-focus=""
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={`Default model for ${harness}: ${value ? modelLabel(value) : 'Harness default'}`}
        className="k2-focus inline-flex items-center gap-1 rounded-[var(--radius-field)] border border-[var(--color-border)] px-2 py-0.5 type-control text-[var(--color-text-primary)] hover:bg-[var(--color-wash-1)]"
        onClick={() => setOpen((v) => !v)}
      >
        {value ? modelLabel(value) : 'Harness default'}
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      {error ? (
        <span role="alert" className="type-hint text-[var(--color-status-error-soft)]">
          {error}
        </span>
      ) : null}
      {open ? (
        <ModelPicker
          anchor={btn.current}
          rows={rows}
          checked={value}
          state={loaded ? 'ready' : 'loading'}
          footer={MODEL_PICKER_WORDS.settingFooter}
          label={`Default model for ${harness}`}
          testId="settings-model-picker"
          onPick={(v) => void setDefault(harness, v)}
          onClose={(refocus) => {
            setOpen(false)
            if (refocus) btn.current?.focus()
          }}
        />
      ) : null}
    </div>
  )
}
