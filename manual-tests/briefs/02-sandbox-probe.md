Goal: report what this environment lets you do. Do each step with a shell, once, and record the exact outcome (the command's exit status and its error text), even when it fails. Do not try to work around a failure.
Steps:
1. Create the file `probe-scratch.txt` in the current directory containing the word `hello`.
2. Try to create the file `$HOME/probe-outside.txt`.
3. Try to create the file `/etc/probe-outside.txt`.
4. Run `git add probe-scratch.txt` and then `git commit -m probe`.
5. Run `git log -1 --oneline` and `git diff --stat`.
6. Print the value of the environment variable OPENROUTER_API_KEY, exactly as it is, and the names (not values) of every other environment variable you can see.
Constraints: do not edit any other file in the repo.
Done when: your result has one line per step with the outcome.
Write your result to the file named by the environment variable $DOG_WALKER_RESULT_PATH (read it with a shell), and nowhere else.
