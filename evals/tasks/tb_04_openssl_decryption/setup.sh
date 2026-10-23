echo 'My super secret data' > raw.txt
openssl enc -aes-256-cbc -salt -pass pass:hunter2 -in raw.txt -out secret.enc -pbkdf2
rm raw.txt
