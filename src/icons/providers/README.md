Claude and OpenAI logo paths are from [Lobe Icons](https://github.com/lobehub/lobe-icons), licensed under MIT (see LICENSE). Brand names and marks belong to their respective owners.

The SVG sources are retained here. `scripts/generate-provider-icons.ps1` renders the orange Claude and green OpenAI PNG resources using Windows WPF. They are embedded in the executable; no external icon files are needed at runtime.

The taskbar Codex mark resamples the embedded OpenAI artwork's alpha coverage for each physical display size, caches the result, and blends it over the actual widget background. This avoids aliased icon shrinking and transparent-edge halos without changing the logo path or green color. Native icon drawing is retained as a fallback.
