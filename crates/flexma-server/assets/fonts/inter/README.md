# Inter for server renders

Static instances of the Inter build the web app ships
(`@fontsource-variable/inter`, from Google Fonts), downloaded from the Google
Fonts css2 API (`family=Inter:ital,wght@…`, Inter v20). The server loads them
before system fonts (`render::base_fontdb`), so screenshots and exports break
lines and place text exactly like the editor. When the web app's Inter
changes, download these again.

License: SIL Open Font License 1.1 (`OFL.txt`).
