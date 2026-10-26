# Distribución

## Releases

Cada etiqueta `v*` dispara `.github/workflows/release.yml`: corre los tests, compila en
release y publica `pega-windows-x64.zip` (con `pega.exe`, `pega-cli.exe` y `pega-ocr.exe`)
más su SHA-256.

```bat
git tag v0.1.0
git push origin v0.1.0
```

Los tres ejecutables van juntos en la misma carpeta: `pega.exe` busca a `pega-ocr.exe` a su
lado para el OCR opcional.

## Firma de código (SmartScreen)

Sin firma, Windows SmartScreen avisa al abrir un .exe descargado. Para proyectos de código
abierto hay firma gratuita con [SignPath Foundation](https://signpath.org) (la usa Ditto):

1. El repositorio tiene que ser público y con licencia de código abierto (MIT ya está).
2. Solicitar en <https://signpath.org/apply> con el enlace al repo.
3. Al aprobarse, agregar al workflow el paso de firma de SignPath sobre los tres .exe antes
   de empaquetar.

## winget

Tras la primera release pública, abrir un PR en
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) con los manifiestos de
`packaging/winget/` (cambiar versión, URL y SHA-256 por los de la release). También se puede
generar con `wingetcreate new <url-del-zip>`.
