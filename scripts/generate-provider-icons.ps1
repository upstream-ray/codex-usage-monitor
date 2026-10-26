# Rebuild the embedded PNG resources from the attributed SVG sources.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationCore, WindowsBase
$IconDirectory = Join-Path $PSScriptRoot '..\src\icons\providers'
foreach ($Provider in @(@{Name='claude'; Color='#D97757'}, @{Name='openai'; Color='#10A37F'}, @{Name='antigravity'; Color='#4285F4'})) {
    [xml]$Svg = Get-Content -Encoding UTF8 -Raw -LiteralPath (Join-Path $IconDirectory ($Provider.Name + '.svg'))
    $Geometry = [Windows.Media.Geometry]::Parse($Svg.svg.path.d).Clone()
    $Geometry.Transform = [Windows.Media.ScaleTransform]::new(64.0 / 24, 64.0 / 24)
    $Brush = [Windows.Media.BrushConverter]::new().ConvertFromString($Provider.Color)
    $Visual = [Windows.Media.DrawingVisual]::new()
    $Context = $Visual.RenderOpen()
    $Context.DrawGeometry($Brush, $null, $Geometry)
    $Context.Close()
    $Bitmap = [Windows.Media.Imaging.RenderTargetBitmap]::new(64, 64, 96, 96, [Windows.Media.PixelFormats]::Pbgra32)
    $Bitmap.Render($Visual)
    $Encoder = [Windows.Media.Imaging.PngBitmapEncoder]::new()
    $Encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($Bitmap))
    $Stream = [IO.File]::Create((Join-Path $IconDirectory ($Provider.Name + '.png')))
    try { $Encoder.Save($Stream) } finally { $Stream.Dispose() }
}
