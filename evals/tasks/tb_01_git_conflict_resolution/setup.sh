git init
git config user.name 'eval'
git config user.email 'eval@eval.com'
echo 'line1' > file.txt
git add file.txt
git commit -m 'init'
git checkout -b feature
echo 'line2 feature' >> file.txt
git commit -am 'feature'
git checkout main
echo 'line2 main' >> file.txt
git commit -am 'main'
git merge feature || true
