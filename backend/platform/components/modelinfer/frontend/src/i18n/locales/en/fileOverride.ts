export default {
  fileOverride: {
    title: 'Card File Overrides: {name}',
    hint: 'Only text scripts under the model card scripts/ directory can be overridden. The original card is never modified. Each run config manages its own overrides, up to {max} files; switching the card version clears all overrides.',
    runningHint: 'This run config is active. Saving does not modify running node files; the override takes effect on the next start.',
    files: 'Script files ({overridden} / {total} overridden)',
    empty: 'This model card has no scripts files to override',
    overridden: 'Overridden',
    editing: 'Editing this run config override',
    copyHint: 'Saving creates an override copied from the original script, scoped to this run config',
    create: 'Create Override',
    restoreOriginal: 'Delete & Restore Original',
    viewOriginal: 'View original card content',
    size: 'Current {size} / limit {max}',
    unsaved: 'Unsaved',
    selectFile: 'Select a script file',
    delete: {
      title: 'Delete File Override',
      content: 'Delete the override for {path}? Future deployments will use the original card file.',
    },
    msg: {
      saved: 'Override saved; it takes effect on the next start',
      deleted: 'Override deleted; the original card script is restored',
      tooLarge: 'Override content exceeds the limit {max}',
    },
  },
} as const
