# The CLI, recorded against the fake Graph: `demo/record.sh`.
Output docs/assets/demo-cli.gif
Source demo/common.tape

Type "mst lists list"
Sleep 400ms
Enter
Sleep 2.5s

Type "mst tasks list --list Finances"
Sleep 400ms
Enter
Sleep 2.5s

Hide
Type "clear"
Enter
Show
Type `mst tasks add "Book dentist" --due "in 2 days" --importance 1`
Sleep 400ms
Enter
Sleep 2s

Type "mst search rent"
Sleep 400ms
Enter
Sleep 2.5s

Hide
Type "clear"
Enter
Show
Type "mst done --since mon"
Sleep 400ms
Enter
Sleep 3s

Hide
Type "clear"
Enter
Show
Type "mst tasks list --format json | jq '.items[0]'"
Sleep 400ms
Enter
Sleep 3.5s

Hide
Type "clear"
Enter
Show
Type "mst done --since yesterday --format csv | cut -d, -f2-4"
Sleep 400ms
Enter
Sleep 3s
