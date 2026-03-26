packer {
  required_plugins {
    amazon = {
      version = ">= 1.3.0"
      source  = "github.com/hashicorp/amazon"
    }
  }
}

variable "aws_region" {
  type    = string
  default = "us-east-1"
}

variable "binary_path" {
  type        = string
  description = "Path to the compiled tdf-iroh-s3 binary"
}

variable "ami_name_prefix" {
  type    = string
  default = "tdf-iroh-s3"
}

source "amazon-ebs" "al2023" {
  ami_name      = "${var.ami_name_prefix}-{{timestamp}}"
  instance_type = "t3.medium"
  region        = var.aws_region

  source_ami_filter {
    filters = {
      name                = "al2023-ami-*-x86_64"
      root-device-type    = "ebs"
      virtualization-type = "hvm"
    }
    most_recent = true
    owners      = ["amazon"]
  }

  ssh_username = "ec2-user"

  tags = {
    Name    = "${var.ami_name_prefix}"
    Builder = "packer"
  }
}

build {
  sources = ["source.amazon-ebs.al2023"]

  provisioner "file" {
    source      = var.binary_path
    destination = "/tmp/tdf-iroh-s3"
  }

  provisioner "file" {
    source      = "files/tdf-iroh-s3.service"
    destination = "/tmp/tdf-iroh-s3.service"
  }

  provisioner "file" {
    source      = "files/bootstrap.sh"
    destination = "/tmp/bootstrap.sh"
  }

  provisioner "shell" {
    script = "scripts/setup-user.sh"
    execute_command = "sudo bash '{{.Path}}'"
  }

  provisioner "shell" {
    script = "scripts/install.sh"
    execute_command = "sudo bash '{{.Path}}'"
  }
}
