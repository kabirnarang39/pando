# Re-record the README demo: run bench/run.sh once (it fetches git/git and fills
# bench/work), start `sapwood server --listen 127.0.0.1:7420 --metadata
# sqlite://bench/work/server/meta.db`, build release binaries, then from the repository root:
#   vhs docs/assets/demo.tape
Output "docs/assets/demo.gif"
Set Shell "bash"
Set FontFamily "Menlo"
Set FontSize 17
Set Width 1180
Set Height 470
Set Padding 28
Set TypingSpeed 35ms
Set Theme { "name": "sapwood", "background": "#2B1B14", "foreground": "#EADBB2", "cursor": "#8DB360", "selection": "#3D2A20", "black": "#2B1B14", "red": "#D98C6A", "green": "#8DB360", "yellow": "#E0B85A", "blue": "#8FB0C9", "magenta": "#C79BB8", "cyan": "#8FC1B5", "white": "#EADBB2", "brightBlack": "#B79F79", "brightRed": "#E8A487", "brightGreen": "#A8CC7A", "brightYellow": "#EDCB7A", "brightBlue": "#A9C6DB", "brightMagenta": "#D9B3CC", "brightCyan": "#A8D4CA", "brightWhite": "#F6EDD3" }

Hide
Type `mkdir -p bench/work/demo && cd bench/work/demo && ln -sfn ../git.git git.git && rm -rf g s && export SAP_SERVER=http://127.0.0.1:7420 PATH="$PWD/../../../target/release":$PATH TIMEFORMAT='%R s' PS1='$ ' && clear`
Enter
Sleep 500ms
Show

Type "# git/git: 82,443 commits. Both servers on this machine."
Enter
Sleep 800ms
Type "time git clone -q file://$PWD/git.git g"
Enter
Wait+Screen@60s /[0-9] s/
Sleep 1.2s
Type "time sap clone http://127.0.0.1:7420 s"
Enter
Wait+Screen@60s /checked out[^$]*[0-9] s/
Sleep 1.5s
Type "# no clone needed to read history or files:"
Enter
Type "time sap log Documentation -n 3 --oneline"
Enter
Sleep 1.8s
Type "time sap cat main GIT-VERSION-GEN | head -3"
Enter
Sleep 3.5s
